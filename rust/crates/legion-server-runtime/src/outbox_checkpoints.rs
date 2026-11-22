//! Checkpoint capture and `BUG-02` rollback outbox handlers for [`crate::outbox_executor::RuntimeOutboxExecutor`].

use std::path::PathBuf;
use std::sync::Arc;

use legion_contracts::ids::{
    CheckpointId, CheckpointRef, CheckpointScopeId, CommandId, NodeId, RunId, ThreadId, TurnItemId,
};
use legion_contracts::orchestration::{
    Checkpoint, CheckpointFileTotals, CheckpointScope, DomainEvent, DomainEventPayload, Run,
    RunStatus, TurnItem, TurnItemBase, TurnItemKind, TurnItemStatus,
};
use legion_orchestrator::Orchestrator;
use legion_scm::checkpoints::rollback::RollbackIntent;

use crate::error::ServerRuntimeError;
use crate::git_checkpoints::{CaptureCheckpointRequest, CapturedCheckpoint, GitCheckpointEngine};
use crate::outbox_extra::wrap_system_event;
use crate::outbox_state::LoadedThreadContext;
use crate::turn_events::commit_runtime_events;

pub(crate) struct RollbackExecParams<'a> {
    pub command_id: &'a CommandId,
    pub checkpoint_id: &'a CheckpointId,
    pub scope_id: &'a CheckpointScopeId,
    pub restore_files: bool,
    pub now_iso: &'a str,
}

struct CheckpointEventInputs<'a> {
    thread_id: &'a ThreadId,
    scope_id: &'a CheckpointScopeId,
    parent: Option<&'a Checkpoint>,
    ordinal: u64,
    next_item_ordinal: u64,
    scope_missing: bool,
    now_iso: &'a str,
}

pub(crate) async fn run_checkpoint_capture(
    orchestrator: &Arc<Orchestrator>,
    engine: GitCheckpointEngine,
    ctx: LoadedThreadContext,
    workspace_root: PathBuf,
    run_id: &RunId,
    scope_id: &CheckpointScopeId,
    now_iso: &str,
) -> Result<(), ServerRuntimeError> {
    if !crate::git_checkpoints_cli::is_git_workspace(&workspace_root) {
        return Ok(());
    }
    let thread_id = ctx.thread.id.clone();
    let run = ctx.find_run(run_id)?.clone();
    let parent = ctx
        .latest_checkpoint_by_scope
        .get(scope_id.as_str())
        .cloned();
    let ordinal = parent
        .as_ref()
        .map_or(1, |cp| cp.ordinal_within_scope.saturating_add(1));
    let parent_ref_owned = parent.as_ref().map(|cp| cp.r#ref.as_str().to_owned());
    let scope_missing = !ctx.scope_ids.contains(scope_id);

    let orch_clone = Arc::clone(orchestrator);
    let thread_id_owned = thread_id.clone();
    let scope_id_owned = scope_id.clone();
    let now_owned = now_iso.to_owned();
    let from_turn = u32::try_from(ordinal.saturating_sub(1)).unwrap_or(0);
    let to_turn = u32::try_from(ordinal).unwrap_or(u32::MAX);

    let captured = tokio::task::spawn_blocking(move || -> Result<_, ServerRuntimeError> {
        let conn = orch_clone
            .sink()
            .db()
            .lock()
            .map_err(|_| ServerRuntimeError::Persistence("db mutex poisoned".to_owned()))?;
        let captured = engine.capture_checkpoint(
            &conn,
            &CaptureCheckpointRequest {
                workspace_root: &workspace_root,
                thread_id: &thread_id_owned,
                scope: scope_id_owned.as_str(),
                ordinal,
                parent_ref: parent_ref_owned.as_deref(),
                from_turn_count: from_turn,
                to_turn_count: to_turn,
                captured_at: &now_owned,
            },
        )?;
        drop(conn);
        if let Ok(scm_ref) =
            legion_scm::checkpoints::checkpoint_ref_for_turn(&thread_id_owned, to_turn)
        {
            let _ = crate::git_checkpoints_cli::run_git(
                &workspace_root,
                &["update-ref", scm_ref.as_str(), &captured.commit_oid],
                &[],
            );
        }
        Ok(captured)
    })
    .await
    .map_err(|e| ServerRuntimeError::Process(format!("checkpoint task failed: {e}")))??;

    let events = build_checkpoint_events(
        run,
        captured,
        &CheckpointEventInputs {
            thread_id: &thread_id,
            scope_id,
            parent: parent.as_ref(),
            ordinal,
            next_item_ordinal: ctx.next_item_ordinal,
            scope_missing,
            now_iso,
        },
    )?;

    commit_runtime_events(
        orchestrator.sink(),
        &thread_id,
        "runtime.checkpoint.capture",
        now_iso,
        events,
        Vec::new(),
    )
}

fn build_checkpoint_events(
    mut run: Run,
    captured: CapturedCheckpoint,
    info: &CheckpointEventInputs<'_>,
) -> Result<Vec<DomainEvent>, ServerRuntimeError> {
    let cp_id = CheckpointId::new(format!("cp-{}-{}", info.scope_id.as_str(), info.ordinal))
        .map_err(|e| ServerRuntimeError::Persistence(e.to_string()))?;
    let cp_ref = CheckpointRef::new(&captured.git_ref)
        .map_err(|e| ServerRuntimeError::Persistence(e.to_string()))?;
    let node_id = run.root_node_id.clone().unwrap_or(
        NodeId::new(format!("node-{}", run.id.as_str()))
            .map_err(|e| ServerRuntimeError::Persistence(e.to_string()))?,
    );
    let mut events = Vec::new();
    if info.scope_missing {
        let scope = CheckpointScope {
            id: info.scope_id.clone(),
            thread_id: info.thread_id.clone(),
            run_id: Some(run.id.clone()),
            node_id: node_id.clone(),
            parent_scope_id: None,
            provider_thread_id: run.provider_thread_id.clone(),
            kind: "root_run".to_owned(),
            ordinal_within_parent: 1,
            advances_app_run_count: true,
            cwd: ".".to_owned(),
            label: None,
            repository_path: None,
            created_at: info.now_iso.to_owned(),
        };
        events.push(wrap_system_event(
            info.thread_id,
            Some(run.id.clone()),
            info.now_iso,
            DomainEventPayload::CheckpointScopeCreated(scope),
        )?);
    }

    let totals = CheckpointFileTotals {
        count: u64::try_from(captured.files.len()).unwrap_or(u64::MAX),
        additions: captured.files.iter().map(|f| f.additions).sum(),
        deletions: captured.files.iter().map(|f| f.deletions).sum(),
    };
    let cp = Checkpoint {
        id: cp_id.clone(),
        thread_id: info.thread_id.clone(),
        scope_id: info.scope_id.clone(),
        run_id: Some(run.id.clone()),
        node_id: node_id.clone(),
        parent_checkpoint_id: info.parent.map(|p| p.id.clone()),
        ordinal_within_scope: info.ordinal,
        app_run_ordinal: Some(run.ordinal),
        r#ref: cp_ref,
        status: "ready".to_owned(),
        files: captured.files.clone(),
        files_total: Some(serde_json::to_value(&totals)?),
        captured_at: info.now_iso.to_owned(),
    };
    events.push(wrap_system_event(
        info.thread_id,
        Some(run.id.clone()),
        info.now_iso,
        DomainEventPayload::CheckpointCaptured(cp),
    )?);

    let cp_item_id = TurnItemId::new(format!("item-cp-{}", cp_id.as_str()))
        .map_err(|e| ServerRuntimeError::Persistence(e.to_string()))?;
    let item = build_checkpoint_turn_item(
        cp_item_id,
        &run,
        &node_id,
        &cp_id,
        info,
        captured.files,
        totals,
    );
    events.push(wrap_system_event(
        info.thread_id,
        Some(run.id.clone()),
        info.now_iso,
        DomainEventPayload::TurnItemUpdated(Box::new(item)),
    )?);

    run.checkpoint_id = Some(cp_id);
    events.push(wrap_system_event(
        info.thread_id,
        Some(run.id.clone()),
        info.now_iso,
        DomainEventPayload::RunUpdated {
            event_type: "run.updated".to_owned(),
            run: Box::new(run),
        },
    )?);
    Ok(events)
}

fn build_checkpoint_turn_item(
    id: TurnItemId,
    run: &Run,
    node_id: &NodeId,
    cp_id: &CheckpointId,
    info: &CheckpointEventInputs<'_>,
    files: Vec<legion_contracts::orchestration::CheckpointFileSummary>,
    totals: CheckpointFileTotals,
) -> TurnItem {
    TurnItem {
        base: TurnItemBase {
            deferred_work: None,
            tool_non_execution_kind: None,
            tool_surface: None,
            tool_icon: None,
            tool_source: None,
            id,
            thread_id: info.thread_id.clone(),
            run_id: Some(run.id.clone()),
            node_id: Some(node_id.clone()),
            provider_thread_id: run.provider_thread_id.clone(),
            provider_turn_id: None,
            native_item_ref: None,
            parent_item_id: None,
            ordinal: info.next_item_ordinal,
            status: TurnItemStatus::Completed,
            title: None,
            started_at: Some(info.now_iso.to_owned()),
            completed_at: Some(info.now_iso.to_owned()),
            updated_at: info.now_iso.to_owned(),
        },
        kind: TurnItemKind::Checkpoint {
            checkpoint_id: cp_id.clone(),
            scope_id: info.scope_id.clone(),
            files,
            files_total: Some(totals),
            files_omitted: None,
        },
    }
}

pub(crate) async fn run_checked_rollback(
    orchestrator: &Arc<Orchestrator>,
    engine: GitCheckpointEngine,
    ctx: LoadedThreadContext,
    workspace_root: PathBuf,
    params: &RollbackExecParams<'_>,
) -> Result<(), ServerRuntimeError> {
    let thread_id = ctx.thread.id.clone();
    let cp = ctx.checkpoint_by_id(params.checkpoint_id).ok_or_else(|| {
        ServerRuntimeError::NotFound(format!("checkpoint `{}` not found", params.checkpoint_id))
    })?;
    let scm_intent = RollbackIntent {
        thread_id: thread_id.clone(),
        command_id: params.command_id.clone(),
        checkpoint_id: params.checkpoint_id.clone(),
        scope_id: Some(params.scope_id.clone()),
        checkpoint_ref: cp.r#ref.clone(),
        restore_files: params.restore_files,
        worktree_is_shared: false,
    };
    let cp_ordinal = cp.app_run_ordinal.unwrap_or(cp.ordinal_within_scope);

    let orch_clone = Arc::clone(orchestrator);
    tokio::task::spawn_blocking(move || -> Result<(), ServerRuntimeError> {
        let conn = orch_clone
            .sink()
            .db()
            .lock()
            .map_err(|_| ServerRuntimeError::Persistence("db mutex poisoned".to_owned()))?;
        let _outcome = engine.rollback_checked(&conn, &workspace_root, &scm_intent, None)?;
        Ok(())
    })
    .await
    .map_err(|e| ServerRuntimeError::Process(format!("rollback task failed: {e}")))??;

    let mut cleared_thread = ctx.thread;
    cleared_thread.rollback_request_id = None;
    cleared_thread.rollback_failure = None;
    params.now_iso.clone_into(&mut cleared_thread.updated_at);
    let mut events = vec![wrap_system_event(
        &thread_id,
        None,
        params.now_iso,
        DomainEventPayload::ThreadState {
            event_type: "thread.metadata-updated".to_owned(),
            thread: Box::new(cleared_thread),
        },
    )?];
    for run in &ctx.runs {
        if run.ordinal > cp_ordinal && run.status != RunStatus::RolledBack {
            let mut rb = run.clone();
            rb.status = RunStatus::RolledBack;
            rb.completed_at = Some(rb.completed_at.unwrap_or_else(|| params.now_iso.to_owned()));
            events.push(wrap_system_event(
                &thread_id,
                Some(rb.id.clone()),
                params.now_iso,
                DomainEventPayload::RunUpdated {
                    event_type: "run.updated".to_owned(),
                    run: Box::new(rb),
                },
            )?);
        }
    }
    commit_runtime_events(
        orchestrator.sink(),
        &thread_id,
        "runtime.checkpoint.rollback-complete",
        params.now_iso,
        events,
        Vec::new(),
    )
}
