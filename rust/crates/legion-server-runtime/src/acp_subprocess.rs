//! Live external ACP subprocess turn runner over stdio NDJSON.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use legion_contracts::ids::{
    CheckpointScopeId, ProviderDriverKind, ProviderSessionId, ProviderThreadId,
};
use legion_contracts::orchestration::{
    DomainEventPayload, ProviderInteractionMode, Run, RunAttempt, RunStatus,
};
use legion_contracts::policy::RuntimeMode;
use legion_orchestrator::sink::{EventSink, EventSubscriber};
use legion_process_ffi::env::SanitizedChildEnv;
use legion_process_ffi::pipes::PipeSetConfig;
use legion_process_ffi::supervisor::{JobProcessSupervisor, SpawnSpec};
use legion_providers::acp::codec::{JsonRpcEnvelope, JsonRpcId};
use legion_providers::acp::protocol::{
    ACP_PROTOCOL_VERSION, ClientCapabilities, FileSystemCapabilities, ImplementationInfo,
    InitializeRequest, InitializeResponse, METHOD_INITIALIZE, METHOD_SESSION_NEW,
    METHOD_SESSION_PROMPT, NewSessionRequest, NewSessionResponse, PromptContentBlock,
    PromptRequest, PromptResponse, StopReason,
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::{Notify, mpsc};

use crate::acp_protocol_io::{
    AcpIoEvent, AcpWriter, decode_session_update, finalize_acp_turn, handle_agent_request,
    spawn_stderr_collector, spawn_stdout_reader,
};
use crate::acp_registry::{AcpDriverSpec, AcpProcessRegistry, ActiveTurnEntry};
use crate::error::ServerRuntimeError;
use crate::turn_events::{TurnEmitContext, commit_runtime_events};

/// Inputs for running one live ACP stdio subprocess turn.
#[derive(Debug, Clone)]
pub struct AcpTurnInput {
    /// Target run record.
    pub run: Run,
    /// Target run attempt record if present.
    pub attempt: Option<RunAttempt>,
    /// Thread runtime permission mode.
    pub runtime_mode: RuntimeMode,
    /// Thread interaction mode.
    pub interaction_mode: ProviderInteractionMode,
    /// Project/worktree working directory.
    pub workspace_root: PathBuf,
    /// User prompt text.
    pub prompt: String,
    /// Existing provider thread ID if already bound.
    pub provider_thread_id: Option<ProviderThreadId>,
    /// ISO-8601 base timestamp.
    pub now_iso: String,
    /// Checkpoint scope identifier queued upon normal completion.
    pub scope_id: CheckpointScopeId,
    /// Next timeline item ordinal.
    pub next_item_ordinal: u64,
    /// Whether to enqueue `ThreadTitleGenerate` upon completion.
    pub queue_title_generate: bool,
}

struct AcpLoopIo<'a> {
    sink: &'a EventSink,
    registry: &'a AcpProcessRegistry,
    input: &'a AcpTurnInput,
    writer: &'a AcpWriter,
    rx: &'a mut mpsc::UnboundedReceiver<AcpIoEvent>,
    sub: &'a mut Option<EventSubscriber>,
    ctx: &'a mut TurnEmitContext,
    acp_session_id: &'a Arc<StdMutex<Option<String>>>,
    interrupted: &'a Arc<AtomicBool>,
    interrupt_notify: &'a Arc<Notify>,
}

struct SpawnedAcpChild {
    supervisor: Arc<StdMutex<JobProcessSupervisor>>,
    token: legion_process_ffi::supervisor::ProcessToken,
    writer: AcpWriter,
    rx: mpsc::UnboundedReceiver<AcpIoEvent>,
    stderr_capture: Option<Arc<StdMutex<String>>>,
}

fn spawn_acp_child(
    spec: &AcpDriverSpec,
    workspace_root: &std::path::Path,
) -> Result<SpawnedAcpChild, ServerRuntimeError> {
    let mut env = SanitizedChildEnv::from_os_env();
    for (k, v) in &spec.env_overrides {
        env.set(k.as_str(), v.as_str());
    }
    let raw_program = spec.program.to_string_lossy();
    let raw_args: Vec<String> = spec
        .args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let (program, args) =
        crate::driver_resolver::resolve_spawn_program_args(&raw_program, &raw_args);
    let spawn_spec = SpawnSpec::new(program)
        .args(args)
        .cwd(workspace_root)
        .env(env)
        .stdio(PipeSetConfig::piped())
        .windows_no_window(spec.windows_no_window);

    let mut sup =
        JobProcessSupervisor::new().map_err(|e| ServerRuntimeError::Process(e.to_string()))?;
    sup.set_kill_on_drop(true);
    let (token, mut pipes) = sup
        .spawn(&spawn_spec)
        .map_err(|e| ServerRuntimeError::Process(e.to_string()))?;
    let supervisor = Arc::new(StdMutex::new(sup));
    let writer = AcpWriter::new(pipes.take_stdin());
    let stdout = pipes
        .take_stdout()
        .ok_or_else(|| ServerRuntimeError::Process("child stdout pipe missing".to_owned()))?;
    let rx = spawn_stdout_reader(stdout);
    let stderr_capture = pipes.take_stderr().map(spawn_stderr_collector);
    Ok(SpawnedAcpChild {
        supervisor,
        token,
        writer,
        rx,
        stderr_capture,
    })
}

/// Spawn and drive one bidirectional ACP stdio subprocess turn to completion or interruption.
pub async fn execute_acp_turn(
    sink: &EventSink,
    registry: &AcpProcessRegistry,
    spec: &AcpDriverSpec,
    input: &AcpTurnInput,
) -> Result<(), ServerRuntimeError> {
    let mut sub = Some(sink.pubsub().subscribe());
    let mut spawned = spawn_acp_child(spec, &input.workspace_root)?;
    let driver_slug = spec.driver_slug.as_deref().unwrap_or_else(|| {
        legion_providers::drivers::normalize_driver_slug(input.run.provider_instance_id.as_str())
    });
    let driver = ProviderDriverKind::new(driver_slug)
        .map_err(|e| ServerRuntimeError::Persistence(e.to_string()))?;
    let session_id = ProviderSessionId::new(format!("psess-{}", input.run.id.as_str()))
        .map_err(|e| ServerRuntimeError::Persistence(e.to_string()))?;
    let p_thread_id = match input.provider_thread_id.clone() {
        Some(existing) => existing,
        None => ProviderThreadId::new(format!("pthread-{}", input.run.thread_id.as_str()))
            .map_err(|e| ServerRuntimeError::Persistence(e.to_string()))?,
    };
    let acp_session_id = Arc::new(StdMutex::new(None));
    let interrupted = Arc::new(AtomicBool::new(false));
    let interrupt_notify = Arc::new(Notify::new());

    registry.insert(ActiveTurnEntry {
        run_id: input.run.id.clone(),
        thread_id: input.run.thread_id.clone(),
        provider_session_id: session_id.clone(),
        provider_thread_id: p_thread_id.clone(),
        token: spawned.token,
        supervisor: Arc::clone(&spawned.supervisor),
        writer: spawned.writer.clone(),
        acp_session_id: Arc::clone(&acp_session_id),
        interrupted: Arc::clone(&interrupted),
        interrupt_notify: Arc::clone(&interrupt_notify),
    });

    let mut ctx = TurnEmitContext::new(
        &input.run,
        driver,
        session_id,
        p_thread_id,
        input.now_iso.clone(),
        input.next_item_ordinal,
    )?;
    let start_events =
        ctx.build_start_events(&input.run, input.attempt.as_ref(), &input.now_iso)?;
    commit_runtime_events(
        sink,
        &input.run.thread_id,
        "runtime.acp.turn-started",
        &input.now_iso,
        start_events,
        Vec::new(),
    )?;

    let mut io = AcpLoopIo {
        sink,
        registry,
        input,
        writer: &spawned.writer,
        rx: &mut spawned.rx,
        sub: &mut sub,
        ctx: &mut ctx,
        acp_session_id: &acp_session_id,
        interrupted: &interrupted,
        interrupt_notify: &interrupt_notify,
    };
    let result = drive_acp_protocol(&mut io).await;

    registry.remove(&input.run.id);
    spawned.writer.close_stdin();
    if let Ok(mut sup_guard) = spawned.supervisor.lock() {
        let _ = sup_guard.terminate_child(spawned.token);
        let _ = sup_guard.terminate_tree(1);
    }

    finalize_acp_turn(
        sink,
        input,
        &mut ctx,
        result,
        &interrupted,
        spawned.stderr_capture.as_ref(),
    )
}

async fn drive_acp_protocol(io: &mut AcpLoopIo<'_>) -> Result<StopReason, ServerRuntimeError> {
    let init_req = InitializeRequest {
        protocol_version: ACP_PROTOCOL_VERSION,
        client_capabilities: ClientCapabilities {
            fs: FileSystemCapabilities {
                read_text_file: false,
                write_text_file: false,
            },
            terminal: false,
        },
        client_info: Some(ImplementationInfo {
            name: "legion-server-runtime".to_owned(),
            title: Some("LegionCode Server Runtime".to_owned()),
            version: crate::SERVER_RUNTIME_VERSION.to_owned(),
        }),
        meta: None,
    };
    let init_id = io.writer.send_request(METHOD_INITIALIZE, Some(&init_req))?;
    let _init_resp: InitializeResponse = await_response(io, init_id).await?;

    let new_req = NewSessionRequest {
        cwd: io.input.workspace_root.to_string_lossy().into_owned(),
        mcp_servers: Vec::new(),
        meta: None,
    };
    let new_id = io.writer.send_request(METHOD_SESSION_NEW, Some(&new_req))?;
    let new_resp: NewSessionResponse = await_response(io, new_id).await?;
    if let Ok(mut guard) = io.acp_session_id.lock() {
        *guard = Some(new_resp.session_id.clone());
    }

    let prompt_req = PromptRequest {
        session_id: new_resp.session_id,
        prompt: vec![PromptContentBlock::Text {
            text: io.input.prompt.clone(),
        }],
        meta: None,
    };
    let prompt_id = io
        .writer
        .send_request(METHOD_SESSION_PROMPT, Some(&prompt_req))?;
    let prompt_resp: PromptResponse = await_response(io, prompt_id).await?;
    Ok(prompt_resp.stop_reason)
}

async fn await_response<T: DeserializeOwned>(
    io: &mut AcpLoopIo<'_>,
    expected_id: JsonRpcId,
) -> Result<T, ServerRuntimeError> {
    loop {
        if io.interrupted.load(Ordering::SeqCst) {
            return Err(ServerRuntimeError::Process("turn interrupted".to_owned()));
        }
        let next = tokio::select! {
            () = io.interrupt_notify.notified() => {
                return Err(ServerRuntimeError::Process("turn interrupted".to_owned()));
            }
            ev = recv_pubsub_interrupt(io.sub, &io.input.run.id) => {
                if ev {
                    let _ = io.registry.interrupt_matching(
                        &io.input.run.thread_id,
                        Some(&io.input.run.id),
                        None,
                        None,
                    );
                    return Err(ServerRuntimeError::Process("turn interrupted".to_owned()));
                }
                continue;
            }
            ev = io.rx.recv() => ev,
        };
        match next {
            Some(AcpIoEvent::Envelope(env)) => match env {
                JsonRpcEnvelope::Response(resp) if resp.id == expected_id => {
                    if let Some(err) = resp.error {
                        return Err(ServerRuntimeError::Process(format!(
                            "ACP JSON-RPC error {}: {}",
                            err.code, err.message
                        )));
                    }
                    let val = resp.result.unwrap_or(Value::Null);
                    return serde_json::from_value::<T>(val)
                        .map_err(|e| ServerRuntimeError::Process(e.to_string()));
                }
                JsonRpcEnvelope::Response(_) => {}
                JsonRpcEnvelope::Request(req) => {
                    handle_agent_request(
                        io.writer,
                        &req,
                        io.input.runtime_mode,
                        io.input.interaction_mode,
                    )?;
                }
                JsonRpcEnvelope::Notification(notif) => {
                    if let Some(update) =
                        decode_session_update(&notif.method, notif.params.as_ref())
                    {
                        let events = io.ctx.translate_update(&update.update, &io.input.now_iso)?;
                        if !events.is_empty() {
                            commit_runtime_events(
                                io.sink,
                                &io.input.run.thread_id,
                                "runtime.acp.stream",
                                &io.input.now_iso,
                                events,
                                Vec::new(),
                            )?;
                        }
                    }
                }
            },
            Some(AcpIoEvent::Eof) | None => {
                return Err(ServerRuntimeError::Process(
                    "ACP subprocess closed stdout before responding".to_owned(),
                ));
            }
            Some(AcpIoEvent::Error(msg)) => {
                return Err(ServerRuntimeError::Process(msg));
            }
        }
    }
}

async fn recv_pubsub_interrupt(
    sub: &mut Option<EventSubscriber>,
    run_id: &legion_contracts::ids::RunId,
) -> bool {
    let Some(rx) = sub.as_mut() else {
        return std::future::pending().await;
    };
    let Some(persisted) = rx.recv().await else {
        *sub = None;
        return false;
    };
    matches!(
        &persisted.event.payload,
        DomainEventPayload::RunUpdated { run, .. }
            if &run.id == run_id && matches!(run.status, RunStatus::Interrupted | RunStatus::Cancelled)
    )
}
