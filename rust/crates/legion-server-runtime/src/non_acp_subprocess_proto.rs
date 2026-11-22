//! Protocol encoders, line-reader threads, and approval replies for non-ACP subprocess providers.

use std::io::{BufRead, BufReader, PipeReader, PipeWriter, Write};
use std::sync::{Arc, Mutex as StdMutex};

use legion_contracts::orchestration::ProviderApprovalDecision;
use legion_contracts::policy::RuntimeMode;
use legion_providers::non_acp::NonAcpTurnUpdate;
use legion_providers::non_acp::claude_stream_json::{
    build_claude_interrupt_control_request, build_claude_permission_control_response,
    encode_claude_user_prompt_line, parse_claude_stream_json_line,
};
use legion_providers::non_acp::codex_app_server::{
    CodexAppServerCodec, CodexApprovalDecision, CodexThreadStartParams, CodexTurnStartParams,
    codex_decision_from_approval, encode_apply_patch_approval_response,
    encode_exec_command_approval_response,
};
use legion_providers::non_acp::cursor_and_pi::{
    build_cursor_cancel_request, build_cursor_send_request, build_pi_abort_command,
    build_pi_prompt_command, parse_cursor_sidecar_line, parse_pi_json_line,
};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::error::ServerRuntimeError;

/// Supported non-ACP subprocess driver wire protocols.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NonAcpSubprocessKind {
    /// `OpenAI` Codex App Server NDJSON-RPC (`item/*`, `turn/*`, `*Approval`).
    CodexAppServer,
    /// Anthropic Claude CLI `--output-format stream-json` NDJSON.
    ClaudeStreamJson,
    /// Cursor agent sidecar JSON-lines protocol.
    CursorSidecar,
    /// Pi coding agent CLI `--mode json` / RPC stdin-stdout protocol.
    PiCli,
}

#[derive(Debug)]
pub(crate) enum LineIoEvent {
    Line(String),
    Eof,
    Error(String),
}

#[derive(Debug, Clone)]
pub(crate) struct ChildLineWriter {
    stdin: Arc<StdMutex<Option<PipeWriter>>>,
}

impl ChildLineWriter {
    pub fn new(stdin: Option<PipeWriter>) -> Self {
        Self {
            stdin: Arc::new(StdMutex::new(stdin)),
        }
    }

    pub fn write_line(&self, line_or_frame: &str) -> Result<(), ServerRuntimeError> {
        let mut guard = self
            .stdin
            .lock()
            .map_err(|_| ServerRuntimeError::Process("child stdin mutex poisoned".to_owned()))?;
        let Some(writer) = guard.as_mut() else {
            return Err(ServerRuntimeError::Process(
                "child stdin is closed".to_owned(),
            ));
        };
        writer.write_all(line_or_frame.as_bytes())?;
        if !line_or_frame.ends_with('\n') {
            writer.write_all(b"\n")?;
        }
        writer.flush()?;
        Ok(())
    }

    pub fn close(&self) {
        if let Ok(mut guard) = self.stdin.lock() {
            let _ = guard.take();
        }
    }
}

pub(crate) fn spawn_line_reader(stdout: PipeReader) -> mpsc::UnboundedReceiver<LineIoEvent> {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut buf = String::new();
        loop {
            buf.clear();
            match reader.read_line(&mut buf) {
                Ok(0) => {
                    let _ = tx.send(LineIoEvent::Eof);
                    return;
                }
                Ok(_) => {
                    let trimmed = buf.trim_end_matches(['\r', '\n']).to_owned();
                    if tx.send(LineIoEvent::Line(trimmed)).is_err() {
                        return;
                    }
                }
                Err(err) => {
                    let _ = tx.send(LineIoEvent::Error(err.to_string()));
                    return;
                }
            }
        }
    });
    rx
}

pub(crate) fn write_initial_prompt(
    kind: NonAcpSubprocessKind,
    writer: &ChildLineWriter,
    codex_codec: &mut CodexAppServerCodec,
    prompt: &str,
    cwd: &str,
    model: &str,
    runtime_mode: RuntimeMode,
) -> Result<(Option<u64>, Option<String>), ServerRuntimeError> {
    match kind {
        NonAcpSubprocessKind::CodexAppServer => {
            let (init_id, init_line) = codex_codec
                .encode_initialize(
                    "legion-server-runtime",
                    "LegionCode",
                    crate::SERVER_RUNTIME_VERSION,
                    true,
                )
                .map_err(|e| ServerRuntimeError::Process(e.to_string()))?;
            writer.write_line(&init_line)?;
            let initialized_line = codex_codec
                .encode_initialized()
                .map_err(|e| ServerRuntimeError::Process(e.to_string()))?;
            writer.write_line(&initialized_line)?;
            let (approval_policy, sandbox) = match runtime_mode {
                RuntimeMode::FullAccess => ("never", "danger-full-access"),
                RuntimeMode::Auto | RuntimeMode::AutoAcceptEdits => {
                    ("on-request", "workspace-write")
                }
                RuntimeMode::ApprovalRequired => ("untrusted", "read-only"),
            };
            let (ts_id, ts_line) = codex_codec
                .encode_thread_start(&CodexThreadStartParams {
                    cwd: cwd.to_owned(),
                    model: Some(model.to_owned()),
                    approval_policy: approval_policy.to_owned(),
                    sandbox: sandbox.to_owned(),
                    experimental_raw_events: false,
                })
                .map_err(|e| ServerRuntimeError::Process(e.to_string()))?;
            writer.write_line(&ts_line)?;
            let _ = init_id;
            Ok((Some(ts_id), None))
        }
        NonAcpSubprocessKind::ClaudeStreamJson => {
            let line = encode_claude_user_prompt_line(prompt, &[], None)
                .map_err(|e| ServerRuntimeError::Process(e.to_string()))?;
            writer.write_line(&line)?;
            Ok((None, None))
        }
        NonAcpSubprocessKind::CursorSidecar => {
            let req = build_cursor_send_request("req-1", None, prompt, Some(model), false, false);
            writer.write_line(&serde_json::to_string(&req)?)?;
            Ok((None, Some("req-1".to_owned())))
        }
        NonAcpSubprocessKind::PiCli => {
            let cmd = build_pi_prompt_command(Some("pi-1"), prompt, false);
            writer.write_line(&serde_json::to_string(&cmd)?)?;
            Ok((None, Some("pi-1".to_owned())))
        }
    }
}

pub(crate) fn send_codex_turn_start(
    writer: &ChildLineWriter,
    codex_codec: &mut CodexAppServerCodec,
    native_thread_id: &str,
    prompt: &str,
) -> Result<u64, ServerRuntimeError> {
    let (turn_req_id, line) = codex_codec
        .encode_turn_start(&CodexTurnStartParams {
            thread_id: native_thread_id.to_owned(),
            text: prompt.to_owned(),
            effort: None,
        })
        .map_err(|e| ServerRuntimeError::Process(e.to_string()))?;
    writer.write_line(&line)?;
    Ok(turn_req_id)
}

pub(crate) fn parse_line_for_kind(
    kind: NonAcpSubprocessKind,
    codex_codec: &mut CodexAppServerCodec,
    line: &str,
) -> Result<Vec<NonAcpTurnUpdate>, ServerRuntimeError> {
    match kind {
        NonAcpSubprocessKind::CodexAppServer => {
            let with_nl = format!("{line}\n");
            codex_codec
                .push_bytes(with_nl.as_bytes())
                .map_err(|e| ServerRuntimeError::Process(e.to_string()))
        }
        NonAcpSubprocessKind::ClaudeStreamJson => parse_claude_stream_json_line(line)
            .map_err(|e| ServerRuntimeError::Process(e.to_string())),
        NonAcpSubprocessKind::CursorSidecar => {
            parse_cursor_sidecar_line(line).map_err(|e| ServerRuntimeError::Process(e.to_string()))
        }
        NonAcpSubprocessKind::PiCli => {
            parse_pi_json_line(line).map_err(|e| ServerRuntimeError::Process(e.to_string()))
        }
    }
}

pub(crate) fn send_permission_auto_reply(
    kind: NonAcpSubprocessKind,
    writer: &ChildLineWriter,
    request_id: &str,
    tool_name: &str,
    runtime_mode: RuntimeMode,
) {
    let decision = match runtime_mode {
        RuntimeMode::FullAccess | RuntimeMode::Auto | RuntimeMode::AutoAcceptEdits => {
            ProviderApprovalDecision::Accept
        }
        RuntimeMode::ApprovalRequired => ProviderApprovalDecision::Decline,
    };
    match kind {
        NonAcpSubprocessKind::CodexAppServer => {
            let codex_dec: CodexApprovalDecision = codex_decision_from_approval(decision);
            let rpc_id = request_id
                .parse::<i64>()
                .map_or_else(|_| Value::String(request_id.to_owned()), Value::from);
            let encoded = if tool_name.contains("patch") {
                encode_apply_patch_approval_response(&rpc_id, codex_dec)
            } else {
                encode_exec_command_approval_response(&rpc_id, codex_dec)
            };
            if let Ok(line) = encoded {
                let _ = writer.write_line(&line);
            }
        }
        NonAcpSubprocessKind::ClaudeStreamJson => {
            let val = build_claude_permission_control_response(request_id, decision, None);
            if let Ok(line) = serde_json::to_string(&val) {
                let _ = writer.write_line(&line);
            }
        }
        NonAcpSubprocessKind::CursorSidecar | NonAcpSubprocessKind::PiCli => {}
    }
}

pub(crate) fn send_interrupt_frame(
    kind: NonAcpSubprocessKind,
    writer: &ChildLineWriter,
    native_thread_id: Option<&str>,
    native_turn_id: Option<&str>,
) {
    match kind {
        NonAcpSubprocessKind::CodexAppServer => {
            let mut codec = CodexAppServerCodec::new();
            if let (Some(tid), Some(turn_id)) = (native_thread_id, native_turn_id)
                && let Ok((_, line)) = codec.encode_turn_interrupt(tid, turn_id)
            {
                let _ = writer.write_line(&line);
            }
        }
        NonAcpSubprocessKind::ClaudeStreamJson => {
            let frame = build_claude_interrupt_control_request("ctrl-int-1");
            if let Ok(line) = serde_json::to_string(&frame) {
                let _ = writer.write_line(&line);
            }
        }
        NonAcpSubprocessKind::CursorSidecar => {
            let frame =
                build_cursor_cancel_request("cancel-1", native_thread_id.unwrap_or("agent-1"));
            if let Ok(line) = serde_json::to_string(&frame) {
                let _ = writer.write_line(&line);
            }
        }
        NonAcpSubprocessKind::PiCli => {
            let frame = build_pi_abort_command(Some("abort-1"));
            if let Ok(line) = serde_json::to_string(&frame) {
                let _ = writer.write_line(&line);
            }
        }
    }
}
