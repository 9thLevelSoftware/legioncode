//! Integration tests for live `Replay (dev)` turn execution, git checkpoint capture, and `BUG-02` rollback.

use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

use t3_contracts::ids::{
    CheckpointId, CheckpointScopeId, CommandId, MessageId, ProjectId, ProviderInstanceId, ThreadId,
};
use t3_contracts::orchestration::{
    CheckpointRollbackCommand, CreationSource, InitialThreadMessage, ModelSelection,
    OrchestrationActor, OrchestrationCommand, ProviderInteractionMode, RunStatus,
    ThreadCreateCommand,
};
use t3_contracts::policy::RuntimeMode;
use t3_orchestrator::Orchestrator;
use t3_orchestrator::outbox::OutboxWorkerOptions;
use t3_persistence::project_store::{CreateProjectInput, create_project};
use t3_persistence::sqlite::SqliteStore;
use t3_scm::checkpoints::rollback::{RollbackCoordinator, RollbackIntent};
use t3_scm::checkpoints::store::CheckpointStore;
use t3_server_runtime::RuntimeOutboxExecutor;
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn run_git_ok(repo: &Path, args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let out = Command::new("git")
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(args)
        .output()?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!("git {args:?} failed: {stderr}").into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn init_git_repo_with_dirty_file(dir: &Path) -> TestResult {
    let _ = run_git_ok(dir, &["init"])?;
    let _ = run_git_ok(dir, &["config", "user.name", "Test User"])?;
    let _ = run_git_ok(dir, &["config", "user.email", "test@example.com"])?;
    std::fs::write(dir.join("README.md"), "initial committed line\n")?;
    let _ = run_git_ok(dir, &["add", "README.md"])?;
    let _ = run_git_ok(dir, &["commit", "-m", "initial commit"])?;
    std::fs::write(dir.join("dirty.txt"), "pre-turn dirty content\n")?;
    Ok(())
}

fn build_replay_thread_cmd(
    project_id: ProjectId,
    thread_id: ThreadId,
) -> Result<OrchestrationCommand, Box<dyn std::error::Error>> {
    Ok(OrchestrationCommand::ThreadCreate(ThreadCreateCommand {
        created_by: OrchestrationActor::User,
        creation_source: CreationSource::Web,
        command_id: CommandId::new("cmd-create-replay-1")?,
        thread_id,
        project_id,
        title: "New thread".to_owned(),
        model_selection: ModelSelection {
            instance_id: ProviderInstanceId::new("replay-dev")?,
            model: "replay".to_owned(),
            options: None,
        },
        runtime_mode: RuntimeMode::Auto,
        interaction_mode: ProviderInteractionMode::Default,
        branch: None,
        worktree_path: None,
        link_origin: None,
        delegated_from: None,
        imported_native_thread: None,
        initial_message: Some(InitialThreadMessage {
            message_id: MessageId::new("msg-user-1")?,
            text: "Show showcase reply and capture workspace state".to_owned(),
            attachments: Vec::new(),
            context: None,
            title_seed: None,
            scheduled_task_id: None,
            sender_thread_id: None,
            workspace_strategy: None,
        }),
    }))
}

fn verify_store_and_bug02_guard(
    db: &Arc<Mutex<rusqlite::Connection>>,
    thread_id: &ThreadId,
    cp_id: &CheckpointId,
    cp_ref: &str,
) -> TestResult {
    let conn = db
        .lock()
        .map_err(|_| "db mutex poisoned when checking CheckpointStore")?;
    let blob = CheckpointStore::new(&conn)
        .get_diff_blob(thread_id, 0, 1)?
        .ok_or("checkpoint diff blob missing from CheckpointStore")?;
    assert!(blob.diff.contains("pre-turn dirty content"));

    // BUG-02 guard: calling verify_pending_intent BEFORE ThreadRollback command MUST fail.
    let bogus_intent = RollbackIntent {
        thread_id: thread_id.clone(),
        command_id: CommandId::new("cmd-bogus-rollback")?,
        checkpoint_id: cp_id.clone(),
        scope_id: Some(CheckpointScopeId::new("root")?),
        checkpoint_ref: cp_ref.parse()?,
        restore_files: true,
        worktree_is_shared: false,
    };
    assert!(RollbackCoordinator::verify_pending_intent(&conn, &bogus_intent).is_err());
    Ok(())
}

#[tokio::test]
async fn replay_turn_captures_git_checkpoint_and_bug02_rollback_restores_workspace() -> TestResult {
    let repo_dir = TempDir::new()?;
    init_git_repo_with_dirty_file(repo_dir.path())?;

    let store = SqliteStore::open_in_memory()?;
    let project_id = ProjectId::new("proj-replay-1")?;
    let ws_str = repo_dir.path().to_string_lossy().into_owned();
    let _ = create_project(
        store.connection(),
        &CreateProjectInput::new(&ws_str)
            .with_project_id(project_id.as_str())
            .with_title("Replay Test Repo")
            .with_now_iso("2026-11-20T00:00:00.000Z"),
    )?;

    let db = Arc::new(Mutex::new(store.into_connection()));
    let orchestrator = Arc::new(Orchestrator::new(Arc::clone(&db)));
    let executor = RuntimeOutboxExecutor::new(Arc::clone(&orchestrator));
    let worker = orchestrator.manual_outbox_worker(
        executor,
        OutboxWorkerOptions {
            worker_id: "test-replay-worker".to_owned(),
            max_attempts: 1,
            retry_delay_ms: 0,
        },
    );

    let thread_id = ThreadId::new("thread-replay-1")?;
    let create_cmd = build_replay_thread_cmd(project_id, thread_id.clone())?;
    let _ = orchestrator
        .dispatch_command(&create_cmd, "2026-11-20T00:00:01.000Z")
        .await?;
    let drain_stats = worker.drain().await?;
    assert!(drain_stats.completed >= 2);

    let state = orchestrator.load_thread_state(&thread_id)?;
    let thread = state.thread.ok_or("thread should exist after turn")?;
    assert_eq!(
        thread.title,
        "Show showcase reply and capture workspace state"
    );
    assert_eq!(state.runs.len(), 1);
    assert_eq!(state.runs[0].status, RunStatus::Completed);
    let cp_id = state.runs[0]
        .checkpoint_id
        .clone()
        .ok_or("checkpoint_id must be attached after outbox drain")?;
    assert!(state.known_checkpoints.contains(&cp_id));

    let cp_ref = "refs/t3/orchestration-v2/checkpoints/root/ordinal/1";
    let commit_sha = run_git_ok(repo_dir.path(), &["rev-parse", "--verify", cp_ref])?;
    assert_ne!(commit_sha, "");
    verify_store_and_bug02_guard(&db, &thread_id, &cp_id, cp_ref)?;

    std::fs::write(
        repo_dir.path().join("README.md"),
        "corrupted by later step\n",
    )?;
    std::fs::write(
        repo_dir.path().join("dirty.txt"),
        "overwritten dirty content\n",
    )?;
    std::fs::write(
        repo_dir.path().join("extra-untracked.txt"),
        "should be cleaned\n",
    )?;

    let rollback_cmd = OrchestrationCommand::CheckpointRollback(CheckpointRollbackCommand {
        restore_files: Some(true),
        command_id: CommandId::new("cmd-rollback-1")?,
        thread_id: thread_id.clone(),
        scope_id: CheckpointScopeId::new("root")?,
        checkpoint_id: cp_id,
    });
    let _ = orchestrator
        .dispatch_command(&rollback_cmd, "2026-11-20T00:00:02.000Z")
        .await?;
    let _ = worker.drain().await?;

    assert_eq!(
        std::fs::read_to_string(repo_dir.path().join("README.md"))?,
        "initial committed line\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo_dir.path().join("dirty.txt"))?,
        "pre-turn dirty content\n"
    );
    assert!(!repo_dir.path().join("extra-untracked.txt").exists());

    let after_state = orchestrator.load_thread_state(&thread_id)?;
    let after_thread = after_state.thread.ok_or("thread missing after rollback")?;
    assert!(after_thread.rollback_request_id.is_none());
    assert!(CheckpointId::new("   ").is_err());
    Ok(())
}
