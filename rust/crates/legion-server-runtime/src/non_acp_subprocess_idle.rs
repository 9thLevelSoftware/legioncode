//! Idle-thread unload registry methods (`#41055`) and silent exit validation for non-ACP subprocesses.

use std::sync::{Arc, Mutex as StdMutex};

use legion_providers::drivers::codex_lifecycle::{
    IdleThreadUnloadEntry, provider_thread_runtime_key,
};

use crate::error::ServerRuntimeError;
use crate::turn_events::TurnEmitContext;

use super::NonAcpProcessRegistry;

impl NonAcpProcessRegistry {
    /// Registers or bumps an idle-unload timer slot for `(provider_session_id, provider_thread_id)` (`#41055`).
    pub fn schedule_idle_unload(
        &self,
        provider_session_id: &str,
        app_thread_id: &str,
        provider_thread_id: &str,
    ) -> Result<IdleThreadUnloadEntry, ServerRuntimeError> {
        let mut guard = self.idle_unload.lock().map_err(|_| {
            ServerRuntimeError::Process("idle unload registry mutex poisoned".to_owned())
        })?;
        Ok(guard.schedule_or_bump(provider_session_id, app_thread_id, provider_thread_id))
    }

    /// Claims an idle-unload slot only when `expected_generation` still owns it (`#41055`).
    pub fn take_idle_unload_if_matches(
        &self,
        provider_session_id: &str,
        provider_thread_id: &str,
        expected_generation: u64,
    ) -> Result<Option<IdleThreadUnloadEntry>, ServerRuntimeError> {
        let mut guard = self.idle_unload.lock().map_err(|_| {
            ServerRuntimeError::Process("idle unload registry mutex poisoned".to_owned())
        })?;
        Ok(guard.take_if_generation_matches(
            provider_session_id,
            provider_thread_id,
            expected_generation,
        ))
    }

    /// Lists pending idle-unload entries for `provider_session_id` (`#41055`).
    pub fn pending_idle_unloads(&self, provider_session_id: &str) -> Vec<IdleThreadUnloadEntry> {
        self.idle_unload.lock().map_or_else(
            |_| Vec::new(),
            |g| g.pending_for_session(provider_session_id),
        )
    }

    /// Computes canonical `driver:native_id` runtime key (`#41055`).
    #[must_use]
    pub fn runtime_thread_key(
        provider_thread_id: &str,
        native_driver: Option<&str>,
        native_id: Option<&str>,
    ) -> String {
        provider_thread_runtime_key(provider_thread_id, native_driver, native_id)
    }
}

pub(super) fn check_silent_exit_failure(
    driver_slug: &str,
    early_exit_status: Option<std::process::ExitStatus>,
    stderr_capture: Option<&Arc<StdMutex<String>>>,
    ctx: &TurnEmitContext,
) -> Result<(), ServerRuntimeError> {
    if let Some(status) = early_exit_status
        && !status.success()
    {
        return Err(ServerRuntimeError::Process(format!(
            "provider `{driver_slug}` exited with status {status}"
        )));
    }
    let has_stderr = stderr_capture
        .and_then(|s| s.lock().ok())
        .is_some_and(|s| !s.trim().is_empty());
    if has_stderr && ctx.assistant_text.is_empty() && ctx.tool_items.is_empty() {
        return Err(ServerRuntimeError::Process(format!(
            "provider `{driver_slug}` exited before producing output"
        )));
    }
    Ok(())
}
