//! End-to-end integration tests for `Replay (dev)` (`devReplay`) turns through
//! `Orchestrator` + `EffectOutboxWorker`, `EventPubSub`, git/non-git checkpoints,
//! and fast non-hanging `ProviderProbeService` discovery.

use std::collections::BTreeMap;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use legion_contracts::ids::{CommandId, MessageId, ProjectId, ProviderInstanceId, ThreadId};
use legion_contracts::orchestration::{
    CreationSource, DomainEventPayload, InitialThreadMessage, ModelSelection, OrchestrationActor,
    OrchestrationCommand, ProviderInteractionMode, RunStatus, ThreadCreateCommand, TurnItemKind,
};
use legion_contracts::policy::RuntimeMode;
use legion_orchestrator::Orchestrator;
use legion_orchestrator::outbox::OutboxWorkerOptions;
use legion_persistence::project_store::{CreateProjectInput, create_project};
use legion_persistence::sqlite::SqliteStore;
use legion_providers::core::ServerProviderState;
use legion_providers::drivers::{DISCOVERY_CLI_DRIVERS, ProviderProbeStatus};
use legion_server_runtime::{ProviderProbeService, ProviderProbeTarget, RuntimeOutboxExecutor};
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const MOCK_PROBE_SRC: &str = r#"
use std::io::Read;
use std::net::TcpStream;

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("version") => {
            println!("mock-provider 2.4.1");
        }
        Some("hang") => {
            if let Some(addr) = args.next() {
                if let Ok(mut stream) = TcpStream::connect(addr) {
                    let mut buf = [0_u8; 8];
                    let _ = stream.read(&mut buf);
                }
            }
        }
        _ => std::process::exit(2),
    }
}
"#;

fn compile_mock_probe(dir: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let src = dir.join("mock_probe_cli.rs");
    let bin = dir.join(format!("mock_probe_cli{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(&src, MOCK_PROBE_SRC)?;
    let out = Command::new("rustc")
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()?;
    if !out.status.success() {
        return Err(format!("rustc failed: {}", String::from_utf8_lossy(&out.stderr)).into());
    }
    Ok(bin)
}

fn init_git_repo(dir: &Path) -> TestResult {
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
            return Err("git init/config failed".into());
        }
    }
    std::fs::write(dir.join("README.md"), "initial\n")?;
    let _ = Command::new("git")
        .current_dir(dir)
        .args(["add", "README.md"])
        .output()?;
    let out = Command::new("git")
        .current_dir(dir)
        .args(["commit", "-m", "initial commit"])
        .output()?;
    if !out.status.success() {
        return Err("git commit failed".into());
    }
    Ok(())
}

fn make_dev_replay_thread_cmd(
    thread_id: &str,
    project_id: &str,
    prompt: &str,
) -> Result<OrchestrationCommand, Box<dyn std::error::Error>> {
    Ok(OrchestrationCommand::ThreadCreate(ThreadCreateCommand {
        created_by: OrchestrationActor::User,
        creation_source: CreationSource::Web,
        command_id: CommandId::new(format!("cmd-create-{thread_id}"))?,
        thread_id: ThreadId::new(thread_id)?,
        project_id: ProjectId::new(project_id)?,
        title: "DevReplay Turn".to_owned(),
        model_selection: ModelSelection {
            instance_id: ProviderInstanceId::new("devReplay")?,
            model: "dev-replay".to_owned(),
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
            message_id: MessageId::new(format!("msg-user-{thread_id}"))?,
            text: prompt.to_owned(),
            attachments: Vec::new(),
            context: None,
            title_seed: None,
            scheduled_task_id: None,
            sender_thread_id: None,
            workspace_strategy: None,
            output_schema: None,
        }),
    }))
}

#[tokio::test]
async fn dev_replay_turn_emits_pubsub_events_and_creates_git_checkpoint_refs() -> TestResult {
    let dir = TempDir::new()?;
    let repo_dir = dir.path().join("repo");
    std::fs::create_dir_all(&repo_dir)?;
    init_git_repo(&repo_dir)?;

    let store = SqliteStore::open_in_memory()?;
    let _ = create_project(
        store.connection(),
        &CreateProjectInput::new(repo_dir.to_string_lossy().as_ref())
            .with_project_id("proj-replay-git")
            .with_title("Replay Git")
            .with_now_iso("2026-11-22T00:00:00.000Z"),
    )?;

    let db = Arc::new(Mutex::new(store.into_connection()));
    let orchestrator = Arc::new(Orchestrator::new(Arc::clone(&db)));
    let mut subscriber = orchestrator.subscribe();

    // No manual register_driver call: canonical `"devReplay"` works out of the box.
    let executor = RuntimeOutboxExecutor::new(Arc::clone(&orchestrator));
    let worker = orchestrator.manual_outbox_worker(
        executor,
        OutboxWorkerOptions {
            worker_id: "replay-e2e-worker".to_owned(),
            max_attempts: 1,
            retry_delay_ms: 0,
        },
    );

    let thread_id = ThreadId::new("thread-replay-e2e")?;
    let cmd = make_dev_replay_thread_cmd(
        "thread-replay-e2e",
        "proj-replay-git",
        "Run command tool showcase",
    )?;
    let _ = orchestrator
        .dispatch_command(&cmd, "2026-11-22T00:00:01.000Z")
        .await?;

    let drain_stats = worker.drain().await?;
    assert_eq!(drain_stats.failed_attempts, 0);
    assert!(drain_stats.completed >= 2);

    let mut saw_delta = false;
    let mut saw_final_msg = false;
    let mut saw_tool_item = false;
    let mut saw_completed_run = false;
    let mut saw_checkpoint = false;

    while let Ok(persisted) = subscriber.try_recv() {
        match &persisted.event.payload {
            DomainEventPayload::MessageUpdated(msg) if msg.role == "assistant" => {
                if msg.streaming {
                    saw_delta = true;
                } else if !msg.text.is_empty() {
                    saw_final_msg = true;
                }
            }
            DomainEventPayload::TurnItemUpdated(item)
                if matches!(
                    item.kind,
                    TurnItemKind::CommandExecution { .. } | TurnItemKind::DynamicTool { .. }
                ) =>
            {
                saw_tool_item = true;
            }
            DomainEventPayload::RunUpdated { run, .. } if run.status == RunStatus::Completed => {
                saw_completed_run = true;
            }
            DomainEventPayload::CheckpointCaptured(_) => saw_checkpoint = true,
            _ => {}
        }
    }

    assert!(saw_delta, "expected streaming text delta via EventPubSub");
    assert!(
        saw_final_msg,
        "expected finalized assistant message via EventPubSub"
    );
    assert!(
        saw_tool_item,
        "expected tool activity TurnItemUpdated via EventPubSub"
    );
    assert!(
        saw_completed_run,
        "expected completed RunUpdated via EventPubSub"
    );
    assert!(
        saw_checkpoint,
        "expected CheckpointCaptured via EventPubSub"
    );

    let thread_state = orchestrator.load_thread_state(&thread_id)?;
    assert_eq!(thread_state.runs[0].status, RunStatus::Completed);
    assert!(thread_state.runs[0].checkpoint_id.is_some());

    for git_ref in [
        "refs/legioncode/orchestration-v2/checkpoints/root/ordinal/1",
        "refs/legioncode/checkpoints/thread-replay-e2e/turn-1",
    ] {
        let out = Command::new("git")
            .current_dir(&repo_dir)
            .args(["rev-parse", "--verify", &format!("{git_ref}^{{commit}}")])
            .output()?;
        assert!(out.status.success(), "missing checkpoint ref `{git_ref}`");
    }
    Ok(())
}

#[tokio::test]
async fn dev_replay_turn_handles_non_git_workspace_gracefully() -> TestResult {
    let dir = TempDir::new()?;
    let store = SqliteStore::open_in_memory()?;
    let _ = create_project(
        store.connection(),
        &CreateProjectInput::new(dir.path().to_string_lossy().as_ref())
            .with_project_id("proj-replay-nogit")
            .with_title("Replay Non-Git")
            .with_now_iso("2026-11-22T00:00:00.000Z"),
    )?;
    let db = Arc::new(Mutex::new(store.into_connection()));
    let orchestrator = Arc::new(Orchestrator::new(Arc::clone(&db)));
    let executor = RuntimeOutboxExecutor::new(Arc::clone(&orchestrator));
    let worker = orchestrator.manual_outbox_worker(
        executor,
        OutboxWorkerOptions {
            worker_id: "replay-nogit-worker".to_owned(),
            max_attempts: 1,
            retry_delay_ms: 0,
        },
    );

    let thread_id = ThreadId::new("thread-nogit-e2e")?;
    let cmd = make_dev_replay_thread_cmd("thread-nogit-e2e", "proj-replay-nogit", "hello")?;
    let _ = orchestrator
        .dispatch_command(&cmd, "2026-11-22T00:00:01.000Z")
        .await?;

    let drain_stats = worker.drain().await?;
    assert_eq!(drain_stats.failed_attempts, 0);
    let thread_state = orchestrator.load_thread_state(&thread_id)?;
    assert_eq!(thread_state.runs[0].status, RunStatus::Completed);
    Ok(())
}

#[tokio::test]
async fn provider_probe_discovers_providers_and_enforces_fast_timeout() -> TestResult {
    let dir = TempDir::new()?;
    let bin = compile_mock_probe(dir.path())?;
    let probe = ProviderProbeService::new()?.with_cli_timeout(Duration::from_millis(80));

    let ok_res = probe
        .run_probe(ProviderProbeTarget::Cli {
            executable: bin.to_string_lossy().into_owned(),
            args: vec!["version".to_owned()],
            env: BTreeMap::new(),
            removed_env_keys: Vec::new(),
        })
        .await?;
    assert_eq!(ok_res.status, ProviderProbeStatus::Ready);
    assert_eq!(ok_res.details["version"], "2.4.1");

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?.to_string();
    let hang_res = probe
        .run_probe(ProviderProbeTarget::Cli {
            executable: bin.to_string_lossy().into_owned(),
            args: vec!["hang".to_owned(), addr],
            env: BTreeMap::new(),
            removed_env_keys: Vec::new(),
        })
        .await?;
    assert_eq!(hang_res.status, ProviderProbeStatus::Error);
    assert_eq!(hang_res.details["timedOut"], true);

    let fleet_probe = ProviderProbeService::new()?.with_cli_timeout(Duration::from_millis(500));
    let discovered = fleet_probe.discover_local_providers().await;
    assert_eq!(discovered.len(), DISCOVERY_CLI_DRIVERS.len() + 1);
    assert!(discovered.iter().any(|p| p.instance_id == "antigravity"));
    let dev_replay = discovered
        .iter()
        .find(|p| p.instance_id == "devReplay")
        .ok_or("devReplay missing from discovery snapshot")?;
    assert_eq!(dev_replay.status, ServerProviderState::Ready);
    assert!(dev_replay.installed);
    assert_ne!(dev_replay.models, Vec::new());
    Ok(())
}
