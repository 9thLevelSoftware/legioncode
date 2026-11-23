//! Live [`OutboxEffectExecutor`] dispatching provider turns, steer/restart/detach, checkpoints, titles, and rollbacks.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex as StdMutex};

use legion_contracts::ids::{
    ProviderDriverKind, ProviderSessionId, ProviderThreadId, RunId, ThreadId,
};
use legion_contracts::orchestration::{Run, RunStatus};
use legion_contracts::policy::RuntimeMode;
use legion_orchestrator::Orchestrator;
use legion_orchestrator::outbox::{OutboxEffect, OutboxEffectExecutor, OutboxEffectRequest};
use legion_providers::drivers::normalize_driver_slug;

use crate::acp_registry::AcpProcessRegistry;
use crate::driver_resolver::resolve_dynamic_driver;
use crate::error::ServerRuntimeError;
use crate::git_checkpoints::GitCheckpointEngine;
use crate::non_acp_subprocess::NonAcpProcessRegistry;
use crate::outbox_checkpoints::{RollbackExecParams, run_checked_rollback, run_checkpoint_capture};
use crate::outbox_extra::{
    TitleGenerateRequest, handle_delegated_tasks_stop, handle_session_detach, handle_title_generate,
};
use crate::outbox_state::{LoadedThreadContext, resolve_workspace_root, utc_now_iso};
pub use crate::outbox_turn_dispatch::ProviderDriverMode;
use crate::outbox_turn_dispatch::{TurnDispatchRequest, dispatch_turn_start};
use crate::turn_events::{TurnEmitContext, commit_runtime_events};

/// Live outbox effect executor powering `legion_orchestrator::EffectOutboxWorker`.
#[derive(Clone)]
pub struct RuntimeOutboxExecutor {
    orchestrator: Arc<Orchestrator>,
    git_checkpoints: GitCheckpointEngine,
    acp_registry: AcpProcessRegistry,
    non_acp_registry: NonAcpProcessRegistry,
    drivers: Arc<StdMutex<BTreeMap<String, ProviderDriverMode>>>,
    workspace_overrides: Arc<StdMutex<BTreeMap<String, PathBuf>>>,
    secrets_dir: Arc<StdMutex<Option<PathBuf>>>,
    settings_path: Arc<StdMutex<Option<PathBuf>>>,
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
            settings_path: Arc::new(StdMutex::new(None)),
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

    /// Configure the `settings.json` path used for dynamic `providerInstances` resolution.
    pub fn set_settings_path(&self, path: PathBuf) {
        if let Ok(mut guard) = self.settings_path.lock() {
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

    fn resolve_driver(
        &self,
        instance_id: &str,
        run: &Run,
        runtime_mode: RuntimeMode,
    ) -> Result<ProviderDriverMode, ServerRuntimeError> {
        if let Ok(guard) = self.drivers.lock()
            && let Some(mode) = guard.get(instance_id)
        {
            return Ok(mode.clone());
        }
        let settings = self.settings_path.lock().ok().and_then(|g| g.clone());
        resolve_dynamic_driver(instance_id, run, runtime_mode, settings.as_deref())
    }

    fn load_thread_context_only(
        &self,
        thread_id: &ThreadId,
    ) -> Result<LoadedThreadContext, ServerRuntimeError> {
        let conn = self
            .orchestrator
            .sink()
            .db()
            .lock()
            .map_err(|_| ServerRuntimeError::Persistence("db mutex poisoned".to_owned()))?;
        LoadedThreadContext::load(&conn, thread_id)
    }

    fn load_context_and_workspace(
        &self,
        thread_id: &ThreadId,
    ) -> Result<(LoadedThreadContext, PathBuf), ServerRuntimeError> {
        let overrides = self
            .workspace_overrides
            .lock()
            .ok()
            .map_or_default(|g| g.clone());
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
            other => {
                self.handle_secondary_effect(
                    &effect.thread_id,
                    effect.command_id.as_str(),
                    other,
                    &now_iso,
                )
                .await
            }
        }
    }

    async fn handle_secondary_effect(
        &self,
        thread_id: &ThreadId,
        command_id: &str,
        request: OutboxEffectRequest,
        now_iso: &str,
    ) -> Result<(), ServerRuntimeError> {
        match request {
            OutboxEffectRequest::ProviderSessionDetach {
                provider_session_id,
                detail,
            } => {
                handle_session_detach(
                    &self.orchestrator,
                    &self.non_acp_registry,
                    thread_id,
                    &provider_session_id,
                    detail.as_deref(),
                    now_iso,
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
                    thread_id,
                    reason.as_deref(),
                    now_iso,
                )
                .await
            }
            OutboxEffectRequest::CheckpointCapture { run_id, scope_id } => {
                let (ctx, ws) = self.load_context_and_workspace(thread_id)?;
                run_checkpoint_capture(
                    &self.orchestrator,
                    self.git_checkpoints,
                    ctx,
                    ws,
                    &run_id,
                    &scope_id,
                    now_iso,
                )
                .await
            }
            OutboxEffectRequest::ProviderThreadRollback {
                command_id: rollback_cmd_id,
                checkpoint_id,
                scope_id,
                restore_files,
                ..
            } => {
                self.handle_rollback(
                    thread_id,
                    &rollback_cmd_id,
                    &checkpoint_id,
                    &scope_id,
                    restore_files,
                    now_iso,
                )
                .await
            }
            OutboxEffectRequest::ThreadTitleGenerate {
                kind, message_id, ..
            } => {
                let ctx = self.load_thread_context_only(thread_id)?;
                let title_provider = self.title_provider.lock().ok().and_then(|g| g.clone());
                handle_title_generate(TitleGenerateRequest {
                    orchestrator: &self.orchestrator,
                    ctx,
                    title_provider,
                    thread_id,
                    kind: &kind,
                    command_id,
                    message_id: message_id.as_ref(),
                    now_iso,
                })
                .await
            }
            _ => Ok(()),
        }
    }

    fn handle_interrupt(
        &self,
        thread_id: &ThreadId,
        run_id: Option<&RunId>,
        provider_session_id: Option<&legion_contracts::ids::ProviderSessionId>,
        provider_thread_id: Option<&legion_contracts::ids::ProviderThreadId>,
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
        message_id: &legion_contracts::ids::MessageId,
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
        let driver_mode = self.resolve_driver(
            run.provider_instance_id.as_str(),
            run,
            ctx.thread.runtime_mode,
        )?;
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

    fn record_turn_failure_if_active(
        &self,
        thread_id: &ThreadId,
        run_id: &RunId,
        err: &ServerRuntimeError,
        now_iso: &str,
    ) -> bool {
        let Ok(ctx) = self.load_thread_context_only(thread_id) else {
            return false;
        };
        let Ok(run) = ctx.find_run(run_id).cloned() else {
            return false;
        };
        if !matches!(
            run.status,
            RunStatus::Preparing | RunStatus::Queued | RunStatus::Starting | RunStatus::Running
        ) {
            return true;
        }
        let attempt = ctx.find_attempt(&run);
        let slug = normalize_driver_slug(run.provider_instance_id.as_str());
        let Ok(driver) = ProviderDriverKind::new(slug) else {
            return false;
        };
        let Ok(session_id) = ProviderSessionId::new(format!("psess-{}", run.id.as_str())) else {
            return false;
        };
        let Ok(p_thread_id) = ctx.thread.active_provider_thread_id.clone().map_or_else(
            || ProviderThreadId::new(format!("pthread-{}", run.thread_id.as_str())),
            Ok,
        ) else {
            return false;
        };
        let Ok(mut emit_ctx) = TurnEmitContext::new(
            &run,
            driver,
            session_id,
            p_thread_id,
            now_iso.to_owned(),
            ctx.next_item_ordinal,
        ) else {
            return false;
        };
        let msg = err.to_string();
        let Ok(events) = emit_ctx.build_finish_events_with_error(
            &run,
            attempt.as_ref(),
            RunStatus::Failed,
            Some(&msg),
            now_iso,
        ) else {
            return false;
        };
        commit_runtime_events(
            self.orchestrator.sink(),
            thread_id,
            "runtime.turn.failed",
            now_iso,
            events,
            Vec::new(),
        )
        .is_ok()
    }
}

impl OutboxEffectExecutor for RuntimeOutboxExecutor {
    fn execute<'a>(
        &'a self,
        effect: OutboxEffect,
        will_retry: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            let turn_target = match &effect.request {
                OutboxEffectRequest::ProviderTurnStart { run_id }
                | OutboxEffectRequest::ProviderTurnRestart { run_id, .. } => {
                    Some((effect.thread_id.clone(), run_id.clone()))
                }
                _ => None,
            };
            match self.handle_effect(effect).await {
                Ok(()) => Ok(()),
                Err(err) => {
                    if let Some((ref thread_id, ref run_id)) = turn_target {
                        let permanent =
                            matches!(err, ServerRuntimeError::NotFound(_)) || !will_retry;
                        let now_iso = utc_now_iso();
                        let already_or_recorded = if permanent {
                            self.record_turn_failure_if_active(thread_id, run_id, &err, &now_iso)
                        } else {
                            self.load_thread_context_only(thread_id)
                                .ok()
                                .and_then(|c| c.find_run(run_id).ok().map(|r| r.status))
                                .is_some_and(|st| st == RunStatus::Failed)
                        };
                        if already_or_recorded {
                            return Ok(());
                        }
                    }
                    Err(err.to_string())
                }
            }
        })
    }
}

impl RuntimeOutboxExecutor {
    async fn handle_rollback(
        &self,
        thread_id: &ThreadId,
        command_id: &legion_contracts::ids::CommandId,
        checkpoint_id: &legion_contracts::ids::CheckpointId,
        scope_id: &legion_contracts::ids::CheckpointScopeId,
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
