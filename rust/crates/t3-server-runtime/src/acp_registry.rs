//! Driver specs and active subprocess registry for live ACP stdio turns (`BUG-09`, `BUG-12`).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use t3_contracts::ids::{ProviderSessionId, ProviderThreadId, RunId, ThreadId};
use t3_process_ffi::supervisor::{JobProcessSupervisor, ProcessToken};
use t3_providers::acp::protocol::{CancelNotification, METHOD_SESSION_CANCEL};
use tokio::sync::Notify;

use crate::acp_protocol_io::AcpWriter;
use crate::error::ServerRuntimeError;

/// Specification for spawning an external ACP stdio provider CLI subprocess.
#[derive(Debug, Clone)]
pub struct AcpDriverSpec {
    /// Executable program name or path.
    pub program: OsString,
    /// Fixed CLI arguments preceding execution.
    pub args: Vec<OsString>,
    /// Explicit environment variable overrides layered onto `SanitizedChildEnv`.
    pub env_overrides: BTreeMap<String, String>,
    /// Hide console window on Windows (`CREATE_NO_WINDOW`).
    pub windows_no_window: bool,
}

impl AcpDriverSpec {
    /// Construct a new [`AcpDriverSpec`] from an executable path/name.
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            env_overrides: BTreeMap::new(),
            windows_no_window: true,
        }
    }

    /// Append CLI arguments.
    #[must_use]
    pub fn with_args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Add one environment override applied on top of `SanitizedChildEnv`.
    #[must_use]
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env_overrides.insert(key.into(), value.into());
        self
    }
}

#[derive(Clone)]
pub(crate) struct ActiveTurnEntry {
    pub run_id: RunId,
    pub thread_id: ThreadId,
    pub provider_session_id: ProviderSessionId,
    pub provider_thread_id: ProviderThreadId,
    pub token: ProcessToken,
    pub supervisor: Arc<StdMutex<JobProcessSupervisor>>,
    pub writer: AcpWriter,
    pub acp_session_id: Arc<StdMutex<Option<String>>>,
    pub interrupted: Arc<AtomicBool>,
    pub interrupt_notify: Arc<Notify>,
}

/// Registry of live supervised ACP child turns enabling `BUG-09`/`BUG-12` clean interrupts.
#[derive(Default, Clone)]
pub struct AcpProcessRegistry {
    active: Arc<StdMutex<Vec<ActiveTurnEntry>>>,
}

impl AcpProcessRegistry {
    /// Create an empty [`AcpProcessRegistry`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the count of currently tracked live ACP child processes.
    pub fn active_count(&self) -> usize {
        self.active.lock().map_or(0, |list| list.len())
    }

    /// Send `session/cancel` and terminate matching child process tree(s) via [`JobProcessSupervisor`].
    pub fn interrupt_matching(
        &self,
        thread_id: &ThreadId,
        run_id: Option<&RunId>,
        provider_session_id: Option<&ProviderSessionId>,
        provider_thread_id: Option<&ProviderThreadId>,
    ) -> Result<usize, ServerRuntimeError> {
        let targets: Vec<ActiveTurnEntry> = {
            let guard = self.active.lock().map_err(|_| {
                ServerRuntimeError::Process("ACP registry mutex poisoned".to_owned())
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

        let mut terminated = 0_usize;
        for entry in targets {
            entry.interrupted.store(true, Ordering::SeqCst);
            entry.interrupt_notify.notify_one();
            if let Ok(Some(session_id)) = entry.acp_session_id.lock().map(|g| g.clone()) {
                let _ = entry.writer.send_notification(
                    METHOD_SESSION_CANCEL,
                    Some(&CancelNotification {
                        session_id,
                        meta: None,
                    }),
                );
            }
            entry.writer.close_stdin();
            if let Ok(mut sup) = entry.supervisor.lock() {
                let _ = sup.terminate_child(entry.token);
                let _ = sup.terminate_tree(1);
            }
            terminated = terminated.saturating_add(1);
        }
        Ok(terminated)
    }

    pub(crate) fn insert(&self, entry: ActiveTurnEntry) {
        if let Ok(mut guard) = self.active.lock() {
            guard.push(entry);
        }
    }

    pub(crate) fn remove(&self, run_id: &RunId) {
        if let Ok(mut guard) = self.active.lock() {
            guard.retain(|e| &e.run_id != run_id);
        }
    }
}
