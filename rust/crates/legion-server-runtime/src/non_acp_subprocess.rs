//! Live non-ACP subprocess executor (`CodexAppServer`, `ClaudeStreamJson`, `CursorSidecar`, `PiCli`).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use legion_contracts::ids::{
    CheckpointScopeId, ProviderDriverKind, ProviderSessionId, ProviderThreadId, RunId, ThreadId,
};
use legion_contracts::orchestration::{Run, RunAttempt};
use legion_contracts::policy::RuntimeMode;
use legion_orchestrator::sink::EventSink;
use legion_process_ffi::supervisor::{JobProcessSupervisor, ProcessToken};
use legion_providers::drivers::ProviderLaunchSpec;
use legion_providers::non_acp::codex_app_server::CodexAppServerCodec;
use legion_providers::non_acp::cursor_and_pi::build_pi_prompt_command;
use tokio::sync::Notify;

use crate::error::ServerRuntimeError;
use crate::non_acp_subprocess_loop::{
    SubprocessLoopCtx, drive_non_acp_lines, finalize_non_acp_turn, spawn_supervised_non_acp,
};
use crate::non_acp_subprocess_proto::{
    ChildLineWriter, NonAcpSubprocessKind, send_interrupt_frame, write_initial_prompt,
};
use crate::non_acp_turn_sink::NonAcpStreamSummary;
use crate::turn_events::{TurnEmitContext, commit_runtime_events};

/// Inputs for executing a live non-ACP subprocess provider turn.
#[derive(Debug, Clone)]
pub struct NonAcpSubprocessInput {
    /// Subprocess protocol variant.
    pub kind: NonAcpSubprocessKind,
    /// Target orchestration run.
    pub run: Run,
    /// Target run attempt if present.
    pub attempt: Option<RunAttempt>,
    /// Thread permission mode.
    pub runtime_mode: RuntimeMode,
    /// Workspace directory.
    pub workspace_root: PathBuf,
    /// User prompt text.
    pub prompt: String,
    /// Existing provider thread ID if bound.
    pub provider_thread_id: Option<ProviderThreadId>,
    /// ISO-8601 timestamp.
    pub now_iso: String,
    /// Checkpoint scope identifier queued upon normal completion.
    pub scope_id: CheckpointScopeId,
    /// Next timeline item ordinal.
    pub next_item_ordinal: u64,
    /// Whether to queue `ThreadTitleGenerate` upon completion.
    pub queue_title_generate: bool,
}

#[derive(Clone)]
struct ActiveNonAcpEntry {
    run_id: RunId,
    thread_id: ThreadId,
    provider_session_id: ProviderSessionId,
    provider_thread_id: ProviderThreadId,
    kind: NonAcpSubprocessKind,
    token: ProcessToken,
    supervisor: Arc<StdMutex<JobProcessSupervisor>>,
    writer: ChildLineWriter,
    native_thread_id: Arc<StdMutex<Option<String>>>,
    native_turn_id: Arc<StdMutex<Option<String>>>,
    interrupted: Arc<AtomicBool>,
    interrupt_notify: Arc<Notify>,
}

/// Registry of active non-ACP child turns enabling steer, interrupt, and detach (`BUG-09`, `BUG-12`).
#[derive(Default, Clone)]
pub struct NonAcpProcessRegistry {
    active: Arc<StdMutex<Vec<ActiveNonAcpEntry>>>,
}

impl NonAcpProcessRegistry {
    /// Create an empty [`NonAcpProcessRegistry`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the count of live non-ACP child processes currently tracked.
    pub fn active_count(&self) -> usize {
        self.active.lock().map_or(0, |list| list.len())
    }

    /// Steer an active turn matching `thread_id` / `run_id` when the protocol supports live steer.
    pub fn steer_matching(&self, thread_id: &ThreadId, run_id: Option<&RunId>, text: &str) -> bool {
        let target = self.active.lock().ok().and_then(|list| {
            list.iter()
                .find(|e| &e.thread_id == thread_id && run_id.is_none_or(|id| &e.run_id == id))
                .cloned()
        });
        let Some(entry) = target else {
            return false;
        };
        match entry.kind {
            NonAcpSubprocessKind::CodexAppServer => {
                let tid = entry
                    .native_thread_id
                    .lock()
                    .ok()
                    .and_then(|g| g.clone())
                    .unwrap_or_else(|| entry.provider_thread_id.as_str().to_owned());
                let turn_id = entry
                    .native_turn_id
                    .lock()
                    .ok()
                    .and_then(|g| g.clone())
                    .unwrap_or_else(|| entry.run_id.as_str().to_owned());
                let mut codec = CodexAppServerCodec::new();
                codec
                    .encode_turn_steer(&tid, &turn_id, text)
                    .is_ok_and(|(_, line)| entry.writer.write_line(&line).is_ok())
            }
            NonAcpSubprocessKind::PiCli => {
                let frame = build_pi_prompt_command(Some("steer"), text, true);
                serde_json::to_string(&frame)
                    .is_ok_and(|line| entry.writer.write_line(&line).is_ok())
            }
            NonAcpSubprocessKind::ClaudeStreamJson | NonAcpSubprocessKind::CursorSidecar => false,
        }
    }

    /// Send protocol cancel/abort and kill matching non-ACP child tree(s) via [`JobProcessSupervisor`].
    pub fn interrupt_matching(
        &self,
        thread_id: &ThreadId,
        run_id: Option<&RunId>,
        provider_session_id: Option<&ProviderSessionId>,
        provider_thread_id: Option<&ProviderThreadId>,
    ) -> Result<usize, ServerRuntimeError> {
        let targets: Vec<ActiveNonAcpEntry> = {
            let guard = self.active.lock().map_err(|_| {
                ServerRuntimeError::Process("non-ACP registry mutex poisoned".to_owned())
            })?;
            guard
                .iter()
                .filter(|entry| {
                    if &entry.thread_id != thread_id {
                        return false;
                    }
                    if let Some(id) = run_id {
                        &entry.run_id == id
                    } else if let Some(id) = provider_session_id {
                        &entry.provider_session_id == id
                    } else if let Some(id) = provider_thread_id {
                        &entry.provider_thread_id == id
                    } else {
                        true
                    }
                })
                .cloned()
                .collect()
        };
        let count = targets.len();
        for entry in targets {
            entry.interrupted.store(true, Ordering::SeqCst);
            entry.interrupt_notify.notify_one();
            let nt = entry.native_thread_id.lock().ok().and_then(|g| g.clone());
            let nturn = entry.native_turn_id.lock().ok().and_then(|g| g.clone());
            send_interrupt_frame(entry.kind, &entry.writer, nt.as_deref(), nturn.as_deref());
            entry.writer.close();
            if let Ok(mut sup) = entry.supervisor.lock() {
                let _ = sup.terminate_child(entry.token);
                let _ = sup.terminate_tree(1);
            }
        }
        Ok(count)
    }

    /// Terminate and detach any active child matching `provider_session_id`.
    pub fn detach_session(&self, provider_session_id: &ProviderSessionId) -> usize {
        let targets: Vec<ActiveNonAcpEntry> = self.active.lock().map_or_else(
            |_| Vec::new(),
            |list| {
                list.iter()
                    .filter(|e| &e.provider_session_id == provider_session_id)
                    .cloned()
                    .collect()
            },
        );
        let count = targets.len();
        for entry in targets {
            entry.interrupted.store(true, Ordering::SeqCst);
            entry.interrupt_notify.notify_one();
            entry.writer.close();
            if let Ok(mut sup) = entry.supervisor.lock() {
                let _ = sup.terminate_child(entry.token);
                let _ = sup.terminate_tree(1);
            }
        }
        count
    }

    fn insert(&self, entry: ActiveNonAcpEntry) {
        if let Ok(mut guard) = self.active.lock() {
            guard.push(entry);
        }
    }

    fn remove(&self, run_id: &RunId) {
        if let Ok(mut guard) = self.active.lock() {
            guard.retain(|e| &e.run_id != run_id);
        }
    }
}

/// Spawn and execute one live non-ACP subprocess turn against `ProviderLaunchSpec`.
pub async fn execute_non_acp_subprocess_turn(
    sink: &EventSink,
    registry: &NonAcpProcessRegistry,
    launch: &ProviderLaunchSpec,
    input: &NonAcpSubprocessInput,
) -> Result<(), ServerRuntimeError> {
    let cwd = launch
        .cwd
        .as_deref()
        .map_or_else(|| input.workspace_root.clone(), PathBuf::from);
    let spawned = spawn_supervised_non_acp(launch, &cwd)?;
    let supervisor = spawned.supervisor;
    let token = spawned.token;
    let writer = spawned.writer;
    let mut rx = spawned.rx;
    let stderr_capture = spawned.stderr_capture;

    let driver = ProviderDriverKind::new("codex")
        .map_err(|e| ServerRuntimeError::Persistence(e.to_string()))?;
    let session_id = ProviderSessionId::new(format!("psess-{}", input.run.id.as_str()))
        .map_err(|e| ServerRuntimeError::Persistence(e.to_string()))?;
    let p_thread_id = match input.provider_thread_id.clone() {
        Some(existing) => existing,
        None => ProviderThreadId::new(format!("pthread-{}", input.run.thread_id.as_str()))
            .map_err(|e| ServerRuntimeError::Persistence(e.to_string()))?,
    };
    let native_thread_id = Arc::new(StdMutex::new(None));
    let native_turn_id = Arc::new(StdMutex::new(None));
    let interrupted = Arc::new(AtomicBool::new(false));
    let interrupt_notify = Arc::new(Notify::new());

    registry.insert(ActiveNonAcpEntry {
        run_id: input.run.id.clone(),
        thread_id: input.run.thread_id.clone(),
        provider_session_id: session_id.clone(),
        provider_thread_id: p_thread_id.clone(),
        kind: input.kind,
        token,
        supervisor: Arc::clone(&supervisor),
        writer: writer.clone(),
        native_thread_id: Arc::clone(&native_thread_id),
        native_turn_id: Arc::clone(&native_turn_id),
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
        "runtime.non-acp.turn-started",
        &input.now_iso,
        start_events,
        Vec::new(),
    )?;

    let mut codex_codec = CodexAppServerCodec::new();
    let (codex_ts_req, _) = write_initial_prompt(
        input.kind,
        &writer,
        &mut codex_codec,
        &input.prompt,
        &cwd.to_string_lossy(),
        &input.run.model_selection.model,
        input.runtime_mode,
    )?;

    let mut sub = Some(sink.pubsub().subscribe());
    let mut summary = NonAcpStreamSummary::default();
    let mut loop_ctx = SubprocessLoopCtx {
        rx: &mut rx,
        sub: &mut sub,
        writer: &writer,
        codex_codec: &mut codex_codec,
        codex_ts_req,
        input,
        sink,
        ctx: &mut ctx,
        summary: &mut summary,
        native_thread_id: &native_thread_id,
        interrupted: &interrupted,
        interrupt_notify: &interrupt_notify,
    };
    let stream_res = drive_non_acp_lines(&mut loop_ctx).await;

    registry.remove(&input.run.id);
    writer.close();
    if let Ok(mut sup_guard) = supervisor.lock() {
        let _ = sup_guard.terminate_child(token);
        let _ = sup_guard.terminate_tree(1);
    }

    finalize_non_acp_turn(
        sink,
        input,
        &mut ctx,
        &summary,
        stream_res,
        stderr_capture.as_ref(),
        interrupted.load(Ordering::SeqCst),
    )
}
