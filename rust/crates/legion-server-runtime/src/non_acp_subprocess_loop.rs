//! Async streaming read loop & finalization for non-ACP subprocess turns.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use legion_contracts::ids::CommandId;
use legion_contracts::orchestration::{DomainEventPayload, RunStatus};
use legion_orchestrator::outbox::{OutboxEffectRequest, PendingOutboxEffect};
use legion_orchestrator::sink::{EventSink, EventSubscriber};
use legion_process_ffi::env::SanitizedChildEnv;
use legion_process_ffi::pipes::PipeSetConfig;
use legion_process_ffi::supervisor::{JobProcessSupervisor, ProcessToken, SpawnSpec};
use legion_providers::drivers::ProviderLaunchSpec;
use legion_providers::non_acp::NonAcpTurnUpdate;
use legion_providers::non_acp::codex_app_server::CodexAppServerCodec;
use serde_json::Value;
use tokio::sync::{Notify, mpsc};
use uuid::Uuid;

use crate::acp_protocol_io::{format_process_failure, spawn_stderr_collector};
use crate::error::ServerRuntimeError;
use crate::non_acp_subprocess::NonAcpSubprocessInput;
use crate::non_acp_subprocess_proto::{
    ChildLineWriter, LineIoEvent, NonAcpSubprocessKind, parse_line_for_kind, send_codex_turn_start,
    send_permission_auto_reply, spawn_line_reader,
};
use crate::non_acp_turn_sink::{NonAcpStreamSummary, apply_non_acp_update};
use crate::turn_events::{TurnEmitContext, commit_runtime_events};

pub(crate) struct SpawnedNonAcpChild {
    pub supervisor: Arc<StdMutex<JobProcessSupervisor>>,
    pub token: ProcessToken,
    pub writer: ChildLineWriter,
    pub rx: mpsc::UnboundedReceiver<LineIoEvent>,
    pub stderr_capture: Option<Arc<StdMutex<String>>>,
}

pub(crate) fn spawn_supervised_non_acp(
    launch: &ProviderLaunchSpec,
    cwd: &Path,
) -> Result<SpawnedNonAcpChild, ServerRuntimeError> {
    let mut env = SanitizedChildEnv::from_os_env();
    for removed in &launch.removed_env_keys {
        let _ = env.remove(removed);
    }
    for (k, v) in &launch.env {
        env.set(k.as_str(), v.as_str());
    }
    let spawn_spec = SpawnSpec::new(&launch.executable)
        .args(&launch.args)
        .cwd(cwd)
        .env(env)
        .stdio(PipeSetConfig::piped())
        .windows_no_window(true);

    let mut sup =
        JobProcessSupervisor::new().map_err(|e| ServerRuntimeError::Process(e.to_string()))?;
    sup.set_kill_on_drop(true);
    let (token, mut pipes) = sup
        .spawn(&spawn_spec)
        .map_err(|e| ServerRuntimeError::Process(e.to_string()))?;
    let supervisor = Arc::new(StdMutex::new(sup));
    let writer = ChildLineWriter::new(pipes.take_stdin());
    let stdout = pipes
        .take_stdout()
        .ok_or_else(|| ServerRuntimeError::Process("child stdout pipe missing".to_owned()))?;
    let rx = spawn_line_reader(stdout);
    let stderr_capture = pipes.take_stderr().map(spawn_stderr_collector);
    Ok(SpawnedNonAcpChild {
        supervisor,
        token,
        writer,
        rx,
        stderr_capture,
    })
}

pub(crate) struct SubprocessLoopCtx<'a> {
    pub rx: &'a mut mpsc::UnboundedReceiver<LineIoEvent>,
    pub sub: &'a mut Option<EventSubscriber>,
    pub writer: &'a ChildLineWriter,
    pub codex_codec: &'a mut CodexAppServerCodec,
    pub codex_ts_req: Option<u64>,
    pub input: &'a NonAcpSubprocessInput,
    pub sink: &'a EventSink,
    pub ctx: &'a mut TurnEmitContext,
    pub summary: &'a mut NonAcpStreamSummary,
    pub native_thread_id: &'a Arc<StdMutex<Option<String>>>,
    pub interrupted: &'a Arc<AtomicBool>,
    pub interrupt_notify: &'a Arc<Notify>,
}

pub(crate) async fn drive_non_acp_lines(
    loop_ctx: &mut SubprocessLoopCtx<'_>,
) -> Result<(), ServerRuntimeError> {
    let mut codex_turn_sent = loop_ctx.input.kind != NonAcpSubprocessKind::CodexAppServer;
    loop {
        if loop_ctx.interrupted.load(Ordering::SeqCst) {
            return Err(ServerRuntimeError::Process("turn interrupted".to_owned()));
        }
        let next = tokio::select! {
            () = loop_ctx.interrupt_notify.notified() => {
                return Err(ServerRuntimeError::Process("turn interrupted".to_owned()));
            }
            ev = recv_pubsub_interrupt(loop_ctx.sub, &loop_ctx.input.run.id) => {
                if ev {
                    loop_ctx.interrupted.store(true, Ordering::SeqCst);
                    return Err(ServerRuntimeError::Process("turn interrupted".to_owned()));
                }
                continue;
            }
            item = loop_ctx.rx.recv() => item,
        };
        match next {
            Some(LineIoEvent::Line(line)) => {
                if line.trim().is_empty() {
                    continue;
                }
                if !codex_turn_sent
                    && let Some(ts_id) = loop_ctx.codex_ts_req
                    && let Some(thread_id_str) = extract_codex_thread_id(&line, ts_id)
                {
                    if let Ok(mut guard) = loop_ctx.native_thread_id.lock() {
                        *guard = Some(thread_id_str.clone());
                    }
                    let _ = send_codex_turn_start(
                        loop_ctx.writer,
                        loop_ctx.codex_codec,
                        &thread_id_str,
                        &loop_ctx.input.prompt,
                    )?;
                    codex_turn_sent = true;
                }
                let updates =
                    parse_line_for_kind(loop_ctx.input.kind, loop_ctx.codex_codec, &line)?;
                if !updates.is_empty() {
                    let mut batch = Vec::new();
                    for upd in updates {
                        if let NonAcpTurnUpdate::PermissionRequested {
                            ref request_id,
                            ref tool_name,
                            ..
                        } = upd
                        {
                            send_permission_auto_reply(
                                loop_ctx.input.kind,
                                loop_ctx.writer,
                                request_id,
                                tool_name,
                                loop_ctx.input.runtime_mode,
                            );
                        }
                        if let NonAcpTurnUpdate::SessionStarted {
                            ref provider_session_id,
                            ..
                        } = upd
                            && let Ok(mut g) = loop_ctx.native_thread_id.lock()
                            && g.is_none()
                        {
                            *g = Some(provider_session_id.clone());
                        }
                        batch.extend(apply_non_acp_update(
                            loop_ctx.ctx,
                            loop_ctx.summary,
                            upd,
                            &loop_ctx.input.now_iso,
                            &loop_ctx.input.run.model_selection.model,
                        )?);
                    }
                    if !batch.is_empty() {
                        commit_runtime_events(
                            loop_ctx.sink,
                            &loop_ctx.input.run.thread_id,
                            "runtime.non-acp.stream",
                            &loop_ctx.input.now_iso,
                            batch,
                            Vec::new(),
                        )?;
                    }
                    if loop_ctx.summary.terminal_seen {
                        return Ok(());
                    }
                }
            }
            Some(LineIoEvent::Eof) | None => return Ok(()),
            Some(LineIoEvent::Error(msg)) => return Err(ServerRuntimeError::Process(msg)),
        }
    }
}

pub(crate) fn finalize_non_acp_turn(
    sink: &EventSink,
    input: &NonAcpSubprocessInput,
    ctx: &mut TurnEmitContext,
    summary: &NonAcpStreamSummary,
    stream_res: Result<(), ServerRuntimeError>,
    stderr_capture: Option<&Arc<StdMutex<String>>>,
    was_interrupted: bool,
) -> Result<(), ServerRuntimeError> {
    let final_status = if was_interrupted {
        RunStatus::Interrupted
    } else if stream_res.is_ok() && summary.failure_message.is_none() {
        RunStatus::Completed
    } else {
        RunStatus::Failed
    };
    let finish_events = ctx.build_finish_events(
        &input.run,
        input.attempt.as_ref(),
        final_status,
        &input.now_iso,
    )?;
    let mut followups = Vec::new();
    if final_status == RunStatus::Completed {
        let cmd_id = CommandId::new(format!("cmd-followup-{}", Uuid::new_v4()))
            .map_err(|e| ServerRuntimeError::Persistence(e.to_string()))?;
        followups.push(PendingOutboxEffect {
            id: format!("eff-cp-{}", Uuid::new_v4()),
            command_id: cmd_id.clone(),
            thread_id: input.run.thread_id.clone(),
            request: OutboxEffectRequest::CheckpointCapture {
                run_id: input.run.id.clone(),
                scope_id: input.scope_id.clone(),
            },
            available_at: None,
        });
        if input.queue_title_generate {
            followups.push(PendingOutboxEffect {
                id: format!("eff-title-{}", Uuid::new_v4()),
                command_id: cmd_id,
                thread_id: input.run.thread_id.clone(),
                request: OutboxEffectRequest::ThreadTitleGenerate {
                    kind: "initial".to_owned(),
                    message_id: Some(input.run.user_message_id.clone()),
                },
                available_at: None,
            });
        }
    }
    commit_runtime_events(
        sink,
        &input.run.thread_id,
        "runtime.non-acp.turn-finished",
        &input.now_iso,
        finish_events,
        followups,
    )?;

    if was_interrupted {
        return Ok(());
    }
    if let Err(err) = stream_res {
        return Err(format_process_failure(err, stderr_capture));
    }
    if let Some(ref msg) = summary.failure_message {
        return Err(ServerRuntimeError::Process(msg.clone()));
    }
    Ok(())
}

fn extract_codex_thread_id(line: &str, expected_rpc_id: u64) -> Option<String> {
    let val: Value = serde_json::from_str(line).ok()?;
    let id_matches = val
        .get("id")
        .and_then(Value::as_u64)
        .is_some_and(|n| n == expected_rpc_id);
    if !id_matches {
        return None;
    }
    val.get("result")
        .and_then(|r| {
            r.get("thread")
                .and_then(|t| t.get("id"))
                .or_else(|| r.get("threadId"))
        })
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| Some("codex-thread-1".to_owned()))
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
