//! Bidirectional stdio JSON-RPC 2.0 / ACP NDJSON framing over OS pipes.

use std::io::{PipeReader, PipeWriter, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use legion_contracts::ids::CommandId;
use legion_contracts::orchestration::{ProviderInteractionMode, RunStatus};
use legion_contracts::policy::RuntimeMode;
use legion_orchestrator::outbox::{OutboxEffectRequest, PendingOutboxEffect};
use legion_orchestrator::sink::EventSink;
use legion_providers::acp::codec::{AcpStdioCodec, JsonRpcEnvelope, JsonRpcId, JsonRpcRequest};
use legion_providers::acp::permissions::{RequestPermissionRequest, select_permission_option};
use legion_providers::acp::protocol::{METHOD_SESSION_REQUEST_PERMISSION, StopReason};
use legion_providers::acp::updates::SessionUpdateNotification;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::acp_subprocess::AcpTurnInput;
use crate::error::ServerRuntimeError;
use crate::turn_events::{TurnEmitContext, commit_runtime_events};

/// Events delivered from the child process stdout reader thread.
#[derive(Debug)]
pub enum AcpIoEvent {
    /// Decoded JSON-RPC 2.0 envelope from agent stdout.
    Envelope(JsonRpcEnvelope),
    /// Stdout reached clean EOF.
    Eof,
    /// Codec or pipe read failure.
    Error(String),
}

/// Thread-safe handle to child stdin writer and codec ID generator.
#[derive(Debug, Clone)]
pub struct AcpWriter {
    stdin: Arc<StdMutex<Option<PipeWriter>>>,
    codec: Arc<StdMutex<AcpStdioCodec>>,
}

impl AcpWriter {
    /// Create a new [`AcpWriter`] around an optional child `stdin` pipe.
    pub fn new(stdin: Option<PipeWriter>) -> Self {
        Self {
            stdin: Arc::new(StdMutex::new(stdin)),
            codec: Arc::new(StdMutex::new(AcpStdioCodec::new())),
        }
    }

    /// Encode and write one JSON-RPC request frame, returning its assigned request ID.
    pub fn send_request<P: Serialize>(
        &self,
        method: &str,
        params: Option<&P>,
    ) -> Result<JsonRpcId, ServerRuntimeError> {
        let (id, line) = {
            let mut codec = self
                .codec
                .lock()
                .map_err(|_| ServerRuntimeError::Process("ACP codec mutex poisoned".to_owned()))?;
            codec
                .encode_request(method, params)
                .map_err(|e| ServerRuntimeError::Process(e.to_string()))?
        };
        self.write_line(&line)?;
        Ok(id)
    }

    /// Encode and write a JSON-RPC notification frame.
    pub fn send_notification<P: Serialize>(
        &self,
        method: &str,
        params: Option<&P>,
    ) -> Result<(), ServerRuntimeError> {
        let line = {
            let codec = self
                .codec
                .lock()
                .map_err(|_| ServerRuntimeError::Process("ACP codec mutex poisoned".to_owned()))?;
            codec
                .encode_notification(method, params)
                .map_err(|e| ServerRuntimeError::Process(e.to_string()))?
        };
        self.write_line(&line)
    }

    /// Encode and write a JSON-RPC success response for an agent-initiated request.
    pub fn send_response_ok(&self, id: JsonRpcId, result: Value) -> Result<(), ServerRuntimeError> {
        let line = {
            let codec = self
                .codec
                .lock()
                .map_err(|_| ServerRuntimeError::Process("ACP codec mutex poisoned".to_owned()))?;
            codec
                .encode_response_ok(id, result)
                .map_err(|e| ServerRuntimeError::Process(e.to_string()))?
        };
        self.write_line(&line)
    }

    /// Encode and write a JSON-RPC error response for an unsupported agent request.
    pub fn send_response_err(
        &self,
        id: JsonRpcId,
        code: i64,
        message: &str,
    ) -> Result<(), ServerRuntimeError> {
        let line = {
            let codec = self
                .codec
                .lock()
                .map_err(|_| ServerRuntimeError::Process("ACP codec mutex poisoned".to_owned()))?;
            codec
                .encode_response_err(id, code, message, None)
                .map_err(|e| ServerRuntimeError::Process(e.to_string()))?
        };
        self.write_line(&line)
    }

    /// Close child stdin cleanly to signal EOF to the child process.
    pub fn close_stdin(&self) {
        if let Ok(mut guard) = self.stdin.lock() {
            let _ = guard.take();
        }
    }

    fn write_line(&self, line: &str) -> Result<(), ServerRuntimeError> {
        let mut guard = self
            .stdin
            .lock()
            .map_err(|_| ServerRuntimeError::Process("ACP stdin mutex poisoned".to_owned()))?;
        let Some(writer) = guard.as_mut() else {
            return Err(ServerRuntimeError::Process(
                "child stdin is closed".to_owned(),
            ));
        };
        writer.write_all(line.as_bytes())?;
        writer.flush()?;
        Ok(())
    }
}

/// Spawn a dedicated background reader thread decoding stdout NDJSON via [`AcpStdioCodec`].
pub fn spawn_stdout_reader(mut stdout: PipeReader) -> mpsc::UnboundedReceiver<AcpIoEvent> {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        let mut codec = AcpStdioCodec::new();
        let mut buf = [0_u8; 8192];
        loop {
            match stdout.read(&mut buf) {
                Ok(0) => {
                    match codec.finish() {
                        Ok(Some(env)) => {
                            let _ = tx.send(AcpIoEvent::Envelope(env));
                        }
                        Ok(None) => {}
                        Err(err) => {
                            let _ = tx.send(AcpIoEvent::Error(err.to_string()));
                            return;
                        }
                    }
                    let _ = tx.send(AcpIoEvent::Eof);
                    return;
                }
                Ok(n) => match codec.push_bytes(&buf[..n]) {
                    Ok(envelopes) => {
                        for env in envelopes {
                            if tx.send(AcpIoEvent::Envelope(env)).is_err() {
                                return;
                            }
                        }
                    }
                    Err(err) => {
                        let _ = tx.send(AcpIoEvent::Error(err.to_string()));
                        return;
                    }
                },
                Err(err) => {
                    let _ = tx.send(AcpIoEvent::Error(err.to_string()));
                    return;
                }
            }
        }
    });
    rx
}

/// Spawn a daemon-like drain thread capturing child stderr lines without blocking the child pipe.
pub fn spawn_stderr_collector(mut stderr: PipeReader) -> Arc<StdMutex<String>> {
    let captured = Arc::new(StdMutex::new(String::new()));
    let sink = Arc::clone(&captured);
    std::thread::spawn(move || {
        let mut buf = [0_u8; 4096];
        while let Ok(n) = stderr.read(&mut buf) {
            if n == 0 {
                break;
            }
            let text = String::from_utf8_lossy(&buf[..n]);
            if let Ok(mut guard) = sink.lock()
                && guard.len() < 16 * 1024
            {
                guard.push_str(&text);
            }
        }
    });
    captured
}

/// Handle one agent-initiated JSON-RPC request (`session/request_permission` or `-32601`).
pub fn handle_agent_request(
    writer: &AcpWriter,
    req: &JsonRpcRequest,
    runtime_mode: RuntimeMode,
    interaction_mode: ProviderInteractionMode,
) -> Result<(), ServerRuntimeError> {
    if req.method == METHOD_SESSION_REQUEST_PERMISSION {
        let payload_val = req.params.clone().unwrap_or(Value::Null);
        let parsed: RequestPermissionRequest = serde_json::from_value(payload_val)
            .map_err(|e| ServerRuntimeError::Process(format!("invalid permission request: {e}")))?;
        let resp =
            select_permission_option(&parsed, None, Some(runtime_mode), Some(interaction_mode));
        writer.send_response_ok(req.id.clone(), serde_json::to_value(&resp)?)?;
        return Ok(());
    }
    writer.send_response_err(
        req.id.clone(),
        -32601,
        &format!("unsupported agent request `{}`", req.method),
    )
}

/// Try decoding a `session/update` notification payload into [`SessionUpdateNotification`].
pub fn decode_session_update(
    method: &str,
    params: Option<&Value>,
) -> Option<SessionUpdateNotification> {
    if method != legion_providers::acp::protocol::METHOD_SESSION_UPDATE {
        return None;
    }
    let val = params?;
    serde_json::from_value::<SessionUpdateNotification>(val.clone()).ok()
}

pub(crate) fn format_process_failure(
    err: ServerRuntimeError,
    stderr_capture: Option<&Arc<StdMutex<String>>>,
) -> ServerRuntimeError {
    let stderr_note = stderr_capture
        .and_then(|s| s.lock().ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    if let Some(note) = stderr_note {
        ServerRuntimeError::Process(format!("{err} (stderr: {note})"))
    } else {
        err
    }
}

pub(crate) fn finalize_acp_turn(
    sink: &EventSink,
    input: &AcpTurnInput,
    ctx: &mut TurnEmitContext,
    result: Result<StopReason, ServerRuntimeError>,
    interrupted: &Arc<AtomicBool>,
    stderr_capture: Option<&Arc<StdMutex<String>>>,
) -> Result<(), ServerRuntimeError> {
    let is_interrupted = interrupted.load(Ordering::SeqCst);
    let final_status = match &result {
        Ok(StopReason::Cancelled) => RunStatus::Interrupted,
        Ok(_) => RunStatus::Completed,
        Err(_) if is_interrupted => RunStatus::Interrupted,
        Err(_) => RunStatus::Failed,
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
        "runtime.acp.turn-finished",
        &input.now_iso,
        finish_events,
        followups,
    )?;

    if is_interrupted {
        return Ok(());
    }
    match result {
        Ok(_) => Ok(()),
        Err(err) => Err(format_process_failure(err, stderr_capture)),
    }
}
