//! Live [`OutboxEffectExecutor`] dispatching provider turns, steer/restart/detach, checkpoints, titles, and rollbacks.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex as StdMutex};

use t3_contracts::ids::{RunId, ThreadId};
use t3_orchestrator::Orchestrator;
use t3_orchestrator::outbox::{OutboxEffect, OutboxEffectExecutor, OutboxEffectRequest};

use crate::acp_registry::AcpProcessRegistry;
use crate::error::ServerRuntimeError;
use crate::git_checkpoints::GitCheckpointEngine;
use crate::non_acp_subprocess::NonAcpProcessRegistry;
use crate::outbox_checkpoints::{RollbackExecParams, run_checked_rollback, run_checkpoint_capture};
use crate::outbox_extra::{
    handle_delegated_tasks_stop, handle_session_detach, handle_title_generate,
};
use crate::outbox_state::{LoadedThreadContext, resolve_workspace_root, utc_now_iso};
pub use crate::outbox_turn_dispatch::ProviderDriverMode;
use crate::outbox_turn_dispatch::{TurnDispatchRequest, dispatch_turn_start};

/// Live outbox effect executor powering `t3_orchestrator::EffectOutboxWorker`.
#[derive(Clone)]
pub struct RuntimeOutboxExecutor {
    orchestrator: Arc<Orchestrator>,
    git_checkpoints: GitCheckpointEngine,
    acp_registry: AcpProcessRegistry,
    non_acp_registry: NonAcpProcessRegistry,
    drivers: Arc<StdMutex<BTreeMap<String, ProviderDriverMode>>>,
    workspace_overrides: Arc<StdMutex<BTreeMap<String, PathBuf>>>,
    secrets_dir: Arc<StdMutex<Option<PathBuf>>>,
    title_provider: Arc<StdMutex<Option<ProviderDriverMode>>>,
}

impl RuntimeOutboxExecutor {
    /// Create a new [`RuntimeOutboxExecutor`] wired to `orchestrator`.
    pub fn new(orchestrator: Arc<Orchestrator>) -> Self {
        Self {
            orchestrator,
            git_checkpoints: GitCheckpointEngine::new(),
            acp_registry: AcpProcessRegistry::new(),
            non_acp_registry: NonAcpProcessRegistry::new(),
            drivers: Arc::new(StdMutex::new(BTreeMap::new())),
            workspace_overrides: Arc::new(StdMutex::new(BTreeMap::new())),
            secrets_dir: Arc::new(StdMutex::new(None)),
            title_provider: Arc::new(StdMutex::new(None)),
        }
    }

    /// Access the shared [`AcpProcessRegistry`] tracking live ACP child turns.
    pub const fn acp_registry(&self) -> &AcpProcessRegistry {
        &self.acp_registry
    }

    /// Access the shared [`NonAcpProcessRegistry`] tracking live non-ACP child turns.
    pub const fn non_acp_registry(&self) -> &NonAcpProcessRegistry {
        &self.non_acp_registry
    }

    /// Access the underlying [`Orchestrator`].
    pub fn orchestrator(&self) -> &Arc<Orchestrator> {
        &self.orchestrator
    }

    /// Register a driver backend for a given `provider_instance_id`.
    pub fn register_driver(
        &self,
        provider_instance_id: impl Into<String>,
        mode: ProviderDriverMode,
    ) {
        if let Ok(mut guard) = self.drivers.lock() {
            guard.insert(provider_instance_id.into(), mode);
        }
    }

    /// Configure the secrets directory used by peer delegation stop hooks (`LivePeerMcpSession`).
    pub fn set_secrets_dir(&self, path: PathBuf) {
        if let Ok(mut guard) = self.secrets_dir.lock() {
            *guard = Some(path);
        }
    }

    /// Configure an optional provider-backed driver for `ThreadTitleGenerate`.
    pub fn set_title_provider(&self, mode: ProviderDriverMode) {
        if let Ok(mut guard) = self.title_provider.lock() {
            *guard = Some(mode);
        }
    }

    /// Register an explicit workspace directory override for a `project_id`.
    pub fn register_workspace(&self, project_id: impl Into<String>, workspace_root: PathBuf) {
        if let Ok(mut guard) = self.workspace_overrides.lock() {
            guard.insert(project_id.into(), workspace_root);
        }
    }

    fn resolve_driver(&self, instance_id: &str) -> Result<ProviderDriverMode, ServerRuntimeError> {
        if let Ok(guard) = self.drivers.lock()
            && let Some(mode) = guard.get(instance_id)
        {
            return Ok(mode.clone());
        }
        let normalized = instance_id.trim().to_ascii_lowercase();
        if matches!(
            normalized.as_str(),
            "replay" | "replay-dev" | "dev-replay" | "dev_replay"
        ) {
            return Ok(ProviderDriverMode::DevReplay);
        }
        Err(ServerRuntimeError::NotFound(format!(
            "no runtime provider driver registered for `{instance_id}`"
        )))
    }

    fn load_context_and_workspace(
        &self,
        thread_id: &ThreadId,
    ) -> Result<(LoadedThreadContext, PathBuf), ServerRuntimeError> {
        let overrides = self
            .workspace_overrides
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default();
        let conn = self
            .orchestrator
            .sink()
            .db()
            .lock()
            .map_err(|_| ServerRuntimeError::Persistence("db mutex poisoned".to_owned()))?;
        let ctx = LoadedThreadContext::load(&conn, thread_id)?;
        let ws = resolve_workspace_root(&conn, &ctx.thread, &overrides)?;
        Ok((ctx, ws))
    }

    async fn handle_effect(&self, effect: OutboxEffect) -> Result<(), ServerRuntimeError> {
        let now_iso = utc_now_iso();
        match effect.request {
            OutboxEffectRequest::ProviderTurnStart { run_id } => {
                self.execute_turn_start(&effect.thread_id, &run_id, &now_iso)
                    .await
            }
            OutboxEffectRequest::ProviderTurnInterrupt {
                run_id,
                provider_session_id,
                provider_thread_id,
                ..
            } => self.handle_interrupt(
                &effect.thread_id,
                run_id.as_ref(),
                provider_session_id.as_ref(),
                provider_thread_id.as_ref(),
            ),
            OutboxEffectRequest::ProviderTurnSteer {
                run_id, message_id, ..
            } => self.handle_steer(&effect.thread_id, run_id.as_ref(), &message_id),
            OutboxEffectRequest::ProviderTurnRestart { run_id, .. } => {
                self.handle_interrupt(&effect.thread_id, None, None, None)?;
                self.execute_turn_start(&effect.thread_id, &run_id, &now_iso)
                    .await
            }
            OutboxEffectRequest::ProviderSessionDetach {
                provider_session_id,
                detail,
            } => {
                handle_session_detach(
                    &self.orchestrator,
                    &self.non_acp_registry,
                    &effect.thread_id,
                    &provider_session_id,
                    detail.as_deref(),
                    &now_iso,
                )
                .await
            }
            OutboxEffectRequest::DelegatedTasksStop { reason } => {
                let sec_dir = self.secrets_dir.lock().ok().and_then(|g| g.clone());
                handle_delegated_tasks_stop(
                    &self.orchestrator,
                    &self.acp_registry,
                    &self.non_acp_registry,
                    sec_dir,
                    &effect.thread_id,
                    reason.as_deref(),
                    &now_iso,
                )
                .await
            }
            OutboxEffectRequest::CheckpointCapture { run_id, scope_id } => {
                let (ctx, ws) = self.load_context_and_workspace(&effect.thread_id)?;
                run_checkpoint_capture(
                    &self.orchestrator,
                    self.git_checkpoints,
                    ctx,
                    ws,
                    &run_id,
                    &scope_id,
                    &now_iso,
                )
                .await
            }
            OutboxEffectRequest::ProviderThreadRollback {
                command_id,
                checkpoint_id,
                scope_id,
                restore_files,
                ..
            } => {
                self.handle_rollback(
                    &effect.thread_id,
                    &command_id,
                    &checkpoint_id,
                    &scope_id,
                    restore_files,
                    &now_iso,
                )
                .await
            }
            OutboxEffectRequest::ThreadTitleGenerate { message_id, .. } => {
                let (ctx, _) = self.load_context_and_workspace(&effect.thread_id)?;
                let title_drv = self.title_provider.lock().ok().and_then(|g| g.clone());
                handle_title_generate(
                    &self.orchestrator,
                    ctx,
                    title_drv,
                    &effect.thread_id,
                    message_id.as_ref(),
                    &now_iso,
                )
                .await
            }
            OutboxEffectRequest::Unknown => Ok(()),
        }
    }

    fn handle_interrupt(
        &self,
        thread_id: &ThreadId,
        run_id: Option<&RunId>,
        provider_session_id: Option<&t3_contracts::ids::ProviderSessionId>,
        provider_thread_id: Option<&t3_contracts::ids::ProviderThreadId>,
    ) -> Result<(), ServerRuntimeError> {
        let _ = self.acp_registry.interrupt_matching(
            thread_id,
            run_id,
            provider_session_id,
            provider_thread_id,
        )?;
        let _ = self.non_acp_registry.interrupt_matching(
            thread_id,
            run_id,
            provider_session_id,
            provider_thread_id,
        )?;
        Ok(())
    }

    fn handle_steer(
        &self,
        thread_id: &ThreadId,
        run_id: Option<&RunId>,
        message_id: &t3_contracts::ids::MessageId,
    ) -> Result<(), ServerRuntimeError> {
        let (ctx, _) = self.load_context_and_workspace(thread_id)?;
        let text = ctx.message_text(message_id);
        let _ = self
            .non_acp_registry
            .steer_matching(thread_id, run_id, &text);
        Ok(())
    }

    async fn execute_turn_start(
        &self,
        thread_id: &ThreadId,
        run_id: &RunId,
        now_iso: &str,
    ) -> Result<(), ServerRuntimeError> {
        let (ctx, workspace_root) = self.load_context_and_workspace(thread_id)?;
        let run = ctx.find_run(run_id)?;
        let driver_mode = self.resolve_driver(run.provider_instance_id.as_str())?;
        dispatch_turn_start(
            &self.orchestrator,
            &self.acp_registry,
            &self.non_acp_registry,
            TurnDispatchRequest {
                driver_mode,
                ctx,
                workspace_root,
                run_id,
                now_iso,
            },
        )
        .await
    }
}

impl OutboxEffectExecutor for RuntimeOutboxExecutor {
    fn execute<'a>(
        &'a self,
        effect: OutboxEffect,
        _will_retry: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            self.handle_effect(effect)
                .await
                .map_err(|err| err.to_string())
        })
    }
}

impl RuntimeOutboxExecutor {
    async fn handle_rollback(
        &self,
        thread_id: &ThreadId,
        command_id: &t3_contracts::ids::CommandId,
        checkpoint_id: &t3_contracts::ids::CheckpointId,
        scope_id: &t3_contracts::ids::CheckpointScopeId,
        restore_files: bool,
        now_iso: &str,
    ) -> Result<(), ServerRuntimeError> {
        let (ctx, ws) = self.load_context_and_workspace(thread_id)?;
        run_checked_rollback(
            &self.orchestrator,
            self.git_checkpoints,
            ctx,
            ws,
            &RollbackExecParams {
                command_id,
                checkpoint_id,
                scope_id,
                restore_files,
                now_iso,
            },
        )
        .await
    }
}
