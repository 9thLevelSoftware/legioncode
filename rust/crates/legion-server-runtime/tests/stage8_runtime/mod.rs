//! Shared fixtures and helpers for Stage 8 live integration tests.

pub mod control_plane;
pub mod control_plane_srv;
pub mod preview_cdp;
pub mod providers;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use legion_contracts::ids::{CommandId, MessageId, ProjectId, ProviderInstanceId, ThreadId};
use legion_contracts::orchestration::{
    CreationSource, DomainEventPayload, InitialThreadMessage, ModelSelection, OrchestrationActor,
    OrchestrationCommand, ProviderInteractionMode, ThreadCreateCommand,
};
use legion_contracts::policy::RuntimeMode;
use legion_orchestrator::Orchestrator;
use legion_orchestrator::outbox::OutboxWorkerOptions;
use legion_orchestrator::sink::event_codec::load_thread_events;
use legion_persistence::project_store::{CreateProjectInput, create_project};
use legion_persistence::sqlite::SqliteStore;
use legion_server_runtime::RuntimeOutboxExecutor;
use rusqlite::Connection;
use tempfile::TempDir;

pub type TestResult = Result<(), Box<dyn std::error::Error>>;

pub fn now_unix_ms() -> i64 {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    i64::try_from(ms).unwrap_or(i64::MAX)
}

pub fn compile_mock_rust_bin(
    dir: &Path,
    name: &str,
    src: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let src_path = dir.join(format!("{name}.rs"));
    let bin_path = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(&src_path, src)?;
    let out = Command::new("rustc")
        .arg(&src_path)
        .arg("-o")
        .arg(&bin_path)
        .output()?;
    if !out.status.success() {
        return Err(format!(
            "rustc failed compiling {name}: {}",
            String::from_utf8_lossy(&out.stderr)
        )
        .into());
    }
    Ok(bin_path)
}

fn init_minimal_git_repo(dir: &Path) -> TestResult {
    for args in [
        vec!["init"],
        vec!["config", "user.name", "Test User"],
        vec!["config", "user.email", "test@example.com"],
        vec!["config", "core.autocrlf", "false"],
    ] {
        let out = Command::new("git")
            .current_dir(dir)
            .env("GIT_TERMINAL_PROMPT", "0")
            .args(args)
            .output()?;
        if !out.status.success() {
            return Err("git setup failed".into());
        }
    }
    std::fs::write(dir.join("main.txt"), "hello stage 8\n")?;
    let _ = Command::new("git")
        .current_dir(dir)
        .args(["add", "main.txt"])
        .output()?;
    let _ = Command::new("git")
        .current_dir(dir)
        .args(["commit", "-m", "init"])
        .output()?;
    Ok(())
}

pub struct OrchestratorFixture {
    pub db: Arc<Mutex<Connection>>,
    pub orchestrator: Arc<Orchestrator>,
    pub executor: RuntimeOutboxExecutor,
    pub workspace_dir: PathBuf,
    pub temp_dir: TempDir,
}

pub fn make_orchestrator() -> Result<OrchestratorFixture, Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let ws_dir = dir.path().join("workspace");
    std::fs::create_dir_all(&ws_dir)?;
    init_minimal_git_repo(&ws_dir)?;

    let store = SqliteStore::open_in_memory()?;
    let _ = create_project(
        store.connection(),
        &CreateProjectInput::new(ws_dir.to_string_lossy().as_ref())
            .with_project_id("proj-s8")
            .with_title("Stage 8")
            .with_now_iso("2026-11-20T00:00:00.000Z"),
    )?;
    let db = Arc::new(Mutex::new(store.into_connection()));
    let orchestrator = Arc::new(Orchestrator::new(Arc::clone(&db)));
    let executor = RuntimeOutboxExecutor::new(Arc::clone(&orchestrator));
    Ok(OrchestratorFixture {
        db,
        orchestrator,
        executor,
        workspace_dir: ws_dir,
        temp_dir: dir,
    })
}

pub fn worker_opts() -> OutboxWorkerOptions {
    OutboxWorkerOptions {
        worker_id: "test-s8-worker".to_owned(),
        max_attempts: 1,
        retry_delay_ms: 0,
    }
}

pub async fn dispatch_thread_turn(
    orchestrator: &Orchestrator,
    suffix: &str,
    provider_instance_id: &str,
    model: &str,
    prompt: &str,
) -> Result<ThreadId, Box<dyn std::error::Error>> {
    let thread_id = ThreadId::new(format!("thr-{suffix}"))?;
    let cmd = OrchestrationCommand::ThreadCreate(ThreadCreateCommand {
        created_by: OrchestrationActor::User,
        creation_source: CreationSource::Web,
        command_id: CommandId::new(format!("cmd-create-{suffix}"))?,
        thread_id: thread_id.clone(),
        project_id: ProjectId::new("proj-s8")?,
        title: "New Thread".to_owned(),
        model_selection: ModelSelection {
            instance_id: ProviderInstanceId::new(provider_instance_id)?,
            model: model.to_owned(),
            options: None,
        },
        runtime_mode: RuntimeMode::Auto,
        interaction_mode: ProviderInteractionMode::Default,
        branch: None,
        worktree_path: None,
        link_origin: None,
        delegated_from: None,
        imported_native_thread: None,
        generate_title: None,
        initial_message: Some(InitialThreadMessage {
            message_id: MessageId::new(format!("msg-{suffix}"))?,
            text: prompt.to_owned(),
            attachments: Vec::new(),
            context: None,
            title_seed: None,
            scheduled_task_id: None,
            sender_thread_id: None,
            workspace_strategy: None,
            output_schema: None,
        }),
        title_renamed: None,
        workflow_run: None,
    });
    let _ = orchestrator
        .dispatch_command(&cmd, "2026-11-20T01:00:00.000Z")
        .await?;
    Ok(thread_id)
}

pub fn has_assistant_message_containing(
    db: &Arc<Mutex<Connection>>,
    thread_id: &ThreadId,
    needle: &str,
) -> Result<bool, Box<dyn std::error::Error>> {
    let conn = db.lock().map_err(|_| "db lock")?;
    let events = load_thread_events(&conn, thread_id, 0)?;
    Ok(events.iter().any(|row| {
        matches!(
            &row.event.payload,
            DomainEventPayload::MessageUpdated(msg)
                if msg.role == "assistant" && msg.text.contains(needle)
        )
    }))
}
