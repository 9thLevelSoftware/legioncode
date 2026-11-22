//! Integration tests for live external ACP stdio subprocess turns & `JobProcessSupervisor` interrupts (`BUG-09`, `BUG-12`).

use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

use t3_contracts::ids::{CommandId, MessageId, ProjectId, ProviderInstanceId, RunId, ThreadId};
use t3_contracts::orchestration::{
    CreationSource, DomainEventPayload, InitialThreadMessage, ModelSelection, OrchestrationActor,
    OrchestrationCommand, ProviderInteractionMode, RunInterruptCommand, RunStatus,
    ThreadCreateCommand,
};
use t3_contracts::policy::RuntimeMode;
use t3_orchestrator::Orchestrator;
use t3_orchestrator::outbox::OutboxWorkerOptions;
use t3_persistence::project_store::{CreateProjectInput, create_project};
use t3_persistence::sqlite::SqliteStore;
use t3_server_runtime::{AcpDriverSpec, ProviderDriverMode, RuntimeOutboxExecutor};
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const MOCK_ACP_BIN_SRC: &str = r##"
use std::io::{self, BufRead, Write};

fn emit(line: &str) {
    let mut out = io::stdout().lock();
    let _ = out.write_all(line.as_bytes());
    let _ = out.write_all(b"\n");
    let _ = out.flush();
}

fn json_id(line: &str) -> String {
    if let Some(pos) = line.find("\"id\":") {
        let rest = line[pos + 5..].trim_start();
        let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
        if end > 0 {
            return rest[..end].to_owned();
        }
    }
    "1".to_owned()
}

fn main() {
    let mut hang_mode = false;
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines().map_while(Result::ok);
    while let Some(line) = lines.next() {
        let id = json_id(&line);
        if line.contains("\"method\":\"initialize\"") {
            emit(&format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{{"protocolVersion":1}}}}"#));
        } else if line.contains("\"method\":\"session/new\"") {
            emit(&format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{{"sessionId":"acp-sess-1"}}}}"#));
        } else if line.contains("\"method\":\"session/prompt\"") {
            if line.contains("HANG_UNTIL_KILLED") {
                hang_mode = true;
                emit(r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"acp-sess-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Working before interrupt..."}}}}"#);
                // Block waiting for `session/cancel` or process kill.
                for next in lines.by_ref() {
                    if next.contains("session/cancel") {
                        emit(&format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{{"stopReason":"cancelled"}}}}"#));
                        break;
                    }
                }
                break;
            }
            emit(r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"acp-sess-1","update":{"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"Planning step "}}}}"#);
            emit(r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"acp-sess-1","update":{"sessionUpdate":"tool_call","toolCallId":"tc-1","title":"Check workspace","kind":"execute","status":"in_progress","rawInput":{"cmd":"echo hi"}}}}"#);
            emit(r#"{"jsonrpc":"2.0","id":9001,"method":"session/request_permission","params":{"sessionId":"acp-sess-1","toolCall":{"toolCallId":"tc-1","title":"Check workspace","kind":"execute","status":"pending"},"options":[{"optionId":"opt-allow","name":"Allow once","kind":"allow_once"}]}}"#);
            let perm_reply = lines.next().unwrap_or_default();
            let approved = perm_reply.contains("\"opt-allow\"");
            emit(r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"acp-sess-1","update":{"sessionUpdate":"tool_call_update","toolCallId":"tc-1","kind":"execute","status":"completed","rawOutput":{"ok":true}}}}"#);
            let reply_text = if approved { "ACP permission approved and completed." } else { "Permission missing." };
            emit(&format!(r#"{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"acp-sess-1","update":{{"sessionUpdate":"agent_message_chunk","content":{{"type":"text","text":"{reply_text}"}}}}}}}}"#));
            emit(&format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{{"stopReason":"end_turn"}}}}"#));
        }
    }
    let _ = hang_mode;
}
"##;

fn compile_mock_acp(dir: &Path) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let src_path = dir.join("mock_acp.rs");
    let bin_name = format!("mock_acp{}", std::env::consts::EXE_SUFFIX);
    let bin_path = dir.join(bin_name);
    std::fs::write(&src_path, MOCK_ACP_BIN_SRC)?;
    let out = Command::new("rustc")
        .arg(&src_path)
        .arg("-o")
        .arg(&bin_path)
        .output()?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!("rustc mock_acp failed: {stderr}").into());
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
    std::fs::write(dir.join("main.txt"), "hello acp\n")?;
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

fn make_thread_create(
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
        title: "New thread".to_owned(),
        model_selection: ModelSelection {
            instance_id: ProviderInstanceId::new("mock-acp")?,
            model: "mock-model".to_owned(),
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
async fn acp_live_subprocess_streams_handles_permissions_and_captures_checkpoint() -> TestResult {
    let dir = TempDir::new()?;
    let repo_dir = dir.path().join("repo");
    std::fs::create_dir_all(&repo_dir)?;
    init_minimal_git_repo(&repo_dir)?;
    let bin_path = compile_mock_acp(dir.path())?;

    let store = SqliteStore::open_in_memory()?;
    let _ = create_project(
        store.connection(),
        &CreateProjectInput::new(repo_dir.to_string_lossy().as_ref())
            .with_project_id("proj-acp-1")
            .with_title("ACP Repo")
            .with_now_iso("2026-11-20T00:00:00.000Z"),
    )?;

    let db = Arc::new(Mutex::new(store.into_connection()));
    let orchestrator = Arc::new(Orchestrator::new(Arc::clone(&db)));
    let executor = RuntimeOutboxExecutor::new(Arc::clone(&orchestrator));
    executor.register_driver(
        "mock-acp",
        ProviderDriverMode::AcpCli(AcpDriverSpec::new(&bin_path)),
    );
    let worker = orchestrator.manual_outbox_worker(
        executor.clone(),
        OutboxWorkerOptions {
            worker_id: "test-acp-worker".to_owned(),
            max_attempts: 1,
            retry_delay_ms: 0,
        },
    );

    let thread_id = ThreadId::new("thread-acp-1")?;
    let cmd = make_thread_create("thread-acp-1", "proj-acp-1", "Run ACP check")?;
    let _ = orchestrator
        .dispatch_command(&cmd, "2026-11-20T00:00:01.000Z")
        .await?;

    let drain_stats = worker.drain().await?;
    assert!(drain_stats.completed >= 2);
    assert_eq!(executor.acp_registry().active_count(), 0);

    let state = orchestrator.load_thread_state(&thread_id)?;
    assert_eq!(state.runs.len(), 1);
    assert_eq!(state.runs[0].status, RunStatus::Completed);
    assert!(state.runs[0].checkpoint_id.is_some());

    let conn = db.lock().map_err(|_| "db mutex poisoned")?;
    let events = t3_orchestrator::sink::event_codec::load_thread_events(&conn, &thread_id, 0)?;
    let saw_assistant = events.iter().any(|row| {
        matches!(
            &row.event.payload,
            DomainEventPayload::MessageUpdated(msg)
                if msg.role == "assistant"
                    && !msg.streaming
                    && msg.text == "ACP permission approved and completed."
        )
    });
    assert!(saw_assistant);
    Ok(())
}

#[tokio::test]
async fn acp_interrupt_terminates_live_child_tree_cleanly() -> TestResult {
    let dir = TempDir::new()?;
    let repo_dir = dir.path().join("repo");
    std::fs::create_dir_all(&repo_dir)?;
    init_minimal_git_repo(&repo_dir)?;
    let bin_path = compile_mock_acp(dir.path())?;

    let store = SqliteStore::open_in_memory()?;
    let _ = create_project(
        store.connection(),
        &CreateProjectInput::new(repo_dir.to_string_lossy().as_ref())
            .with_project_id("proj-acp-int")
            .with_title("ACP Interrupt Repo")
            .with_now_iso("2026-11-20T00:00:00.000Z"),
    )?;

    let db = Arc::new(Mutex::new(store.into_connection()));
    let orchestrator = Arc::new(Orchestrator::new(Arc::clone(&db)));
    let executor = RuntimeOutboxExecutor::new(Arc::clone(&orchestrator));
    executor.register_driver(
        "mock-acp",
        ProviderDriverMode::AcpCli(AcpDriverSpec::new(&bin_path)),
    );

    let mut events_sub = orchestrator.subscribe();
    let thread_id = ThreadId::new("thread-acp-int")?;
    let cmd = make_thread_create("thread-acp-int", "proj-acp-int", "HANG_UNTIL_KILLED")?;
    let dispatch = orchestrator
        .dispatch_command(&cmd, "2026-11-20T00:00:01.000Z")
        .await?;
    let run_id = RunId::new(
        dispatch
            .outcome
            .events
            .iter()
            .find_map(|e| e.event.run_id.as_ref().map(|r| r.as_str().to_owned()))
            .ok_or("expected run_id from ThreadCreate")?,
    )?;

    let worker = orchestrator.manual_outbox_worker(
        executor.clone(),
        OutboxWorkerOptions {
            worker_id: "test-acp-turn".to_owned(),
            max_attempts: 1,
            retry_delay_ms: 0,
        },
    );
    let mut turn_handle = tokio::spawn(async move { worker.run_once().await });

    // Wait reactively on `EventPubSub` until child emits first streaming assistant chunk.
    loop {
        tokio::select! {
            ev = events_sub.recv() => {
                let ev = ev.ok_or("event pubsub closed")?;
                if matches!(
                    ev.event.payload,
                    DomainEventPayload::MessageUpdated(ref msg) if msg.role == "assistant"
                ) {
                    break;
                }
            }
            done = &mut turn_handle => {
                return Err(format!("turn_handle exited early before emitting assistant chunk: {done:?}").into());
            }
        }
    }
    assert_eq!(executor.acp_registry().active_count(), 1);

    let interrupt_cmd = OrchestrationCommand::RunInterrupt(RunInterruptCommand {
        command_id: CommandId::new("cmd-int-1")?,
        thread_id: thread_id.clone(),
        run_id,
        reason: Some("user cancelled".to_owned()),
        hold_queue: Some(false),
    });
    let _ = orchestrator
        .dispatch_command(&interrupt_cmd, "2026-11-20T00:00:02.000Z")
        .await?;

    let turn_res = turn_handle.await??;
    assert!(turn_res);
    assert_eq!(executor.acp_registry().active_count(), 0);

    let int_worker = orchestrator.manual_outbox_worker(
        executor.clone(),
        OutboxWorkerOptions {
            worker_id: "test-acp-interrupt".to_owned(),
            max_attempts: 1,
            retry_delay_ms: 0,
        },
    );
    let int_stats = int_worker.drain().await?;
    assert!(int_stats.completed >= 1);
    assert_eq!(executor.acp_registry().active_count(), 0);

    let final_state = orchestrator.load_thread_state(&thread_id)?;
    assert_eq!(final_state.runs[0].status, RunStatus::Interrupted);
    Ok(())
}
