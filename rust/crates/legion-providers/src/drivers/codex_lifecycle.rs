//! Codex app-server unresponsiveness retirement (`#25123`) and native-thread
//! keyed idle-unload tracking (`#41055`).

use std::collections::{BTreeMap, HashMap};

use serde::Deserialize;

/// Deadline (`2 minutes` = `120_000 ms`) after which an unanswered Codex app-server
/// RPC request retires the shared app-server (`#25123`).
pub const CODEX_APP_SERVER_REQUEST_DEADLINE_MS: u64 = 120_000;

/// Failure tag surfaced when an app-server request exceeds [`CODEX_APP_SERVER_REQUEST_DEADLINE_MS`] (`#25123`).
pub const CODEX_APP_SERVER_UNRESPONSIVE_TAG: &str = "CodexAppServerUnresponsiveError";

/// Failure tag surfaced when a caller targets an already-retired Codex app-server (`#25123`).
pub const CODEX_APP_SERVER_RETIRED_TAG: &str = "CodexAppServerRetiredError";

/// Lifecycle error returned by [`CodexAppServerRetirementState`] (`#25123`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexAppServerLifecycleError {
    /// Timed out waiting for a setup or `turn/start` RPC response.
    Unresponsive {
        /// RPC method that missed its deadline (`"config/read"`, `"thread/start"`, etc.).
        method: String,
    },
    /// App-server was previously retired and rejects new work.
    Retired,
}

impl CodexAppServerLifecycleError {
    /// Returns the curated upstream tag identifier (`#25123`).
    #[must_use]
    pub const fn tag(&self) -> &'static str {
        match self {
            Self::Unresponsive { .. } => CODEX_APP_SERVER_UNRESPONSIVE_TAG,
            Self::Retired => CODEX_APP_SERVER_RETIRED_TAG,
        }
    }

    /// Formats the user-visible diagnostic message matching upstream `CodexAdapterV2.ts` (`#25123`).
    #[must_use]
    pub fn display_message(&self) -> String {
        match self {
            Self::Unresponsive { method } => {
                format!("Codex app-server did not answer {method} within 2 minutes.")
            }
            Self::Retired => "Codex app-server was retired and accepts no new work.".to_owned(),
        }
    }
}

/// Tracks retirement state for a shared Codex app-server process (`#25123`).
///
/// Once a request hits [`CODEX_APP_SERVER_REQUEST_DEADLINE_MS`] or the child terminates,
/// the server marks itself retired so buffered events finish draining while new work is refused.
#[derive(Debug, Default, Clone)]
pub struct CodexAppServerRetirementState {
    retired: bool,
    reason: Option<String>,
}

impl CodexAppServerRetirementState {
    /// Creates an active, non-retired app-server state.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            retired: false,
            reason: None,
        }
    }

    /// Returns `true` after the app-server has been retired.
    #[must_use]
    pub const fn is_retired(&self) -> bool {
        self.retired
    }

    /// Returns the retirement reason if recorded.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    /// Retires the app-server due to `method` missing its RPC deadline (`#25123`).
    pub fn mark_unresponsive(&mut self, method: &str) -> CodexAppServerLifecycleError {
        let err = CodexAppServerLifecycleError::Unresponsive {
            method: method.to_owned(),
        };
        self.retired = true;
        self.reason = Some(err.display_message());
        err
    }

    /// Retires the app-server on child process termination.
    pub fn mark_terminated(&mut self, detail: &str) {
        self.retired = true;
        self.reason = Some(detail.to_owned());
    }

    /// Rejects incoming `ensureThread`, `resumeThread`, `startTurn`, or `goalCommand` work once retired (`#25123`).
    ///
    /// # Errors
    ///
    /// Returns [`CodexAppServerLifecycleError::Retired`] when [`Self::is_retired`] is `true`.
    pub const fn reject_if_retired(&self) -> Result<(), CodexAppServerLifecycleError> {
        if self.retired {
            Err(CodexAppServerLifecycleError::Retired)
        } else {
            Ok(())
        }
    }
}

/// Pending idle-unload entry keyed by native `provider_thread_id` so fresh-session
/// scheduled runs never overwrite an earlier native thread's unload timer (`#41055`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdleThreadUnloadEntry {
    /// Owning app thread ID (archived-child checks inspect this).
    pub app_thread_id: String,
    /// Native provider thread ID (`ProviderThreadId`) owning this unload entry.
    pub provider_thread_id: String,
    /// Monotonic timer generation preventing stale timer callbacks from unloading newer generations.
    pub generation: u64,
}

/// Computes `driver:native_id` (or fallback `thread_id`) matching upstream
/// `providerThreadRuntimeKey` (`#41055`).
#[must_use]
pub fn provider_thread_runtime_key(
    provider_thread_id: &str,
    native_driver: Option<&str>,
    native_id: Option<&str>,
) -> String {
    match (native_driver, native_id) {
        (Some(driver), Some(id)) => format!("{driver}:{id}"),
        _ => provider_thread_id.to_owned(),
    }
}

#[derive(Deserialize)]
struct LoadKeyPayload {
    #[serde(rename = "providerThread")]
    provider_thread: String,
}

/// Checks whether an existing `load_key` JSON belongs to the given runtime key (`#41055`).
#[must_use]
pub fn is_load_key_for_provider_thread(
    load_key: Option<&str>,
    provider_thread_id: &str,
    native_driver: Option<&str>,
    native_id: Option<&str>,
) -> bool {
    let Some(raw) = load_key else {
        return true;
    };
    let expected = provider_thread_runtime_key(provider_thread_id, native_driver, native_id);
    serde_json::from_str::<LoadKeyPayload>(raw)
        .is_ok_and(|parsed| parsed.provider_thread == expected)
}

/// Per-session registry of idle provider threads keyed by `provider_thread_id` (`#41055`).
#[derive(Debug, Default, Clone)]
pub struct CodexIdleThreadUnloadRegistry {
    by_session: HashMap<String, BTreeMap<String, IdleThreadUnloadEntry>>,
}

impl CodexIdleThreadUnloadRegistry {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers or advances generation for `(provider_session_id, provider_thread_id)`.
    pub fn schedule_or_bump(
        &mut self,
        provider_session_id: &str,
        app_thread_id: &str,
        provider_thread_id: &str,
    ) -> IdleThreadUnloadEntry {
        let session = self
            .by_session
            .entry(provider_session_id.to_owned())
            .or_default();
        let next_gen = session
            .get(provider_thread_id)
            .map_or(1, |prev| prev.generation.saturating_add(1));
        let entry = IdleThreadUnloadEntry {
            app_thread_id: app_thread_id.to_owned(),
            provider_thread_id: provider_thread_id.to_owned(),
            generation: next_gen,
        };
        session.insert(provider_thread_id.to_owned(), entry.clone());
        entry
    }

    /// Checks whether `generation` still owns the idle slot and removes it when unloading.
    pub fn take_if_generation_matches(
        &mut self,
        provider_session_id: &str,
        provider_thread_id: &str,
        expected_generation: u64,
    ) -> Option<IdleThreadUnloadEntry> {
        let session = self.by_session.get_mut(provider_session_id)?;
        let matches = session
            .get(provider_thread_id)
            .is_some_and(|e| e.generation == expected_generation);
        if !matches {
            return None;
        }
        let removed = session.remove(provider_thread_id);
        if session.is_empty() {
            self.by_session.remove(provider_session_id);
        }
        removed
    }

    /// Lists all pending unload entries on `provider_session_id`, including replaced threads
    /// sharing the same `app_thread_id` (`#41055`).
    #[must_use]
    pub fn pending_for_session(&self, provider_session_id: &str) -> Vec<IdleThreadUnloadEntry> {
        self.by_session
            .get(provider_session_id)
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default()
    }
}
