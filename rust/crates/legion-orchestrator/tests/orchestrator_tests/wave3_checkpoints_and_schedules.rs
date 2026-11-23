use legion_contracts::ids::{
    CheckpointId, CheckpointScopeId, CommandId, NodeId, RunAttemptId, ScheduledTaskId, ThreadId,
};
use legion_contracts::orchestration::{
    CheckpointRollbackCommand, CheckpointScope, OrchestrationCommand, RunStatus, WorkspaceStrategy,
};
use legion_orchestrator::checkpoint_policy::{
    CheckpointCaptureTracker, CheckpointRewindCheck, CheckpointTargetStatus, FolderWalkEntry,
    PRESERVED_CHECKPOINT_SQL_FILTER, RewindMode, can_rewind_to_checkpoint,
    is_rollback_preserved_checkpoint, plan_checkpoint_capture_finalization,
    plan_checkpoint_scope_walk_once, should_reuse_checkpoint_scope, should_skip_pending_rollback,
    worktree_setup_pending,
};
use legion_orchestrator::decider::decide_command;
use legion_orchestrator::scheduled_tasks::{
    ScheduledTaskEditError, WEBHOOK_DELIVERY_LOG_PROMPT_LIMIT,
    ensure_scheduled_task_exists_for_edit, truncate_prompt_to_limit,
    truncate_webhook_delivery_prompt,
};
use legion_orchestrator::sink::projection_apply::DeciderThreadState;

use super::helpers::TestResult;
use super::wave3_helpers::{make_run, make_thread};

#[test]
fn worktree_checkout_unblock_and_shared_timeout_tracker() -> TestResult {
    let mut thread = make_thread("t-wt", "p-1")?;
    thread.worktree_path = None;
    let mut failed_unstarted = make_run("r-1", "t-wt", 1, RunStatus::Failed)?;
    failed_unstarted.workspace_preparation = Some(WorkspaceStrategy::Worktree {
        base_ref: "main".to_owned(),
        branch: None,
        start_from_origin: None,
        repositories: None,
    });
    assert!(worktree_setup_pending(&thread, &[failed_unstarted.clone()]));
    let mut failed_started = failed_unstarted;
    failed_started.started_at = Some("2026-11-22T00:00:00.000Z".to_owned());
    assert!(!worktree_setup_pending(&thread, &[failed_started]));

    let scope = CheckpointScope {
        id: CheckpointScopeId::new("scope-1")?,
        thread_id: thread.id,
        run_id: None,
        node_id: NodeId::new("node-1")?,
        parent_scope_id: None,
        provider_thread_id: None,
        kind: "root_run".to_owned(),
        ordinal_within_parent: 1,
        advances_app_run_count: true,
        cwd: "/tmp/wt-1".to_owned(),
        label: None,
        repository_path: None,
        created_at: "2026-11-22T00:00:00.000Z".to_owned(),
    };
    assert!(should_reuse_checkpoint_scope(Some(&scope), "/tmp/wt-1"));
    assert!(!should_reuse_checkpoint_scope(Some(&scope), "/tmp/project"));

    let tracker = CheckpointCaptureTracker::new();
    let before = tracker.captures_so_far("/repo");
    tracker.note_capture("/repo", true);
    assert!(tracker.should_skip_after_peer_timeout("/repo", before, false));
    assert!(!tracker.should_skip_after_peer_timeout("/repo", before, true));
    tracker.note_capture("/repo", false);
    assert!(!tracker.should_skip_after_peer_timeout("/repo", before, false));
    Ok(())
}

#[test]
fn checkpoint_delete_cancel_rewind_walk_and_schedule_truncation() -> TestResult {
    let mut deleted_thread = make_thread("t-wt", "p-1")?;
    deleted_thread.deleted_at = Some("2026-11-22T01:00:00.000Z".to_owned());
    assert!(should_skip_pending_rollback(&deleted_thread));
    let deleted_state = DeciderThreadState {
        thread: Some(deleted_thread),
        ..DeciderThreadState::default()
    };
    let rb_cmd = OrchestrationCommand::CheckpointRollback(CheckpointRollbackCommand {
        command_id: CommandId::new("cmd-rb")?,
        thread_id: ThreadId::new("t-wt")?,
        checkpoint_id: CheckpointId::new("cp-1")?,
        scope_id: CheckpointScopeId::new("scope-1")?,
        restore_files: Some(false),
    });
    assert!(decide_command(&deleted_state, &rb_cmd, "2026-11-22T01:05:00.000Z").is_err());

    let mut cancelled_run = make_run("r-cap", "t-wt", 2, RunStatus::Cancelled)?;
    cancelled_run.active_attempt_id = Some(RunAttemptId::new("att-1")?);
    let finalized = plan_checkpoint_capture_finalization(
        Some(&cancelled_run),
        cancelled_run.active_attempt_id.as_ref(),
        &CheckpointId::new("cp-2")?,
        "2026-11-22T01:10:00.000Z",
    )
    .ok_or("expected finalized cancelled run")?;
    assert_eq!(finalized.run.status, RunStatus::Cancelled);
    assert_eq!(
        finalized.run.checkpoint_id,
        Some(CheckpointId::new("cp-2")?)
    );
    assert!(!finalized.complete_root_node);

    assert!(can_rewind_to_checkpoint(&CheckpointRewindCheck {
        scope_repository_path: None,
        scope_id: "scope-1",
        checkpoint_scope_id: "scope-1",
        status: CheckpointTargetStatus::Error,
        mode: RewindMode::ConversationOnly,
        linked_project_folder: false,
    }));
    assert!(!can_rewind_to_checkpoint(&CheckpointRewindCheck {
        scope_repository_path: None,
        scope_id: "scope-1",
        checkpoint_scope_id: "scope-1",
        status: CheckpointTargetStatus::Error,
        mode: RewindMode::RestoreWorkspace,
        linked_project_folder: false,
    }));
    assert!(is_rollback_preserved_checkpoint(
        CheckpointTargetStatus::Error,
        true
    ));
    assert!(PRESERVED_CHECKPOINT_SQL_FILTER.contains("'error'"));

    let (git_folders, submodule_inputs) = plan_checkpoint_scope_walk_once(
        "/ws",
        Some("/proj"),
        &[FolderWalkEntry {
            path: "/ws/sub".to_owned(),
            is_git_repository: true,
            repository_path: Some("/proj/sub".to_owned()),
        }],
    );
    assert_eq!(git_folders.len(), 1);
    assert_eq!(submodule_inputs.len(), 2);

    let st_id = ScheduledTaskId::new("sched-1")?;
    assert_eq!(
        ensure_scheduled_task_exists_for_edit(&st_id, false, true),
        Err(ScheduledTaskEditError::DeletedOrMissing(st_id))
    );
    assert_eq!(WEBHOOK_DELIVERY_LOG_PROMPT_LIMIT, 65_536);
    let boundary_emoji = format!("{}😀tail", "a".repeat(9));
    assert_eq!(truncate_prompt_to_limit(&boundary_emoji, 10), "a".repeat(9));
    assert_eq!(truncate_webhook_delivery_prompt("short"), "short");
    Ok(())
}
