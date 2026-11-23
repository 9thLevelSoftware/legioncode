//! Deterministic identifier derivation and UUID allocation for providers.
//!
//! Mirrors `packages/provider-core/src/server/IdAllocator.ts` so every
//! environment derives identical thread, session, run, turn-item, checkpoint,
//! and subagent identifiers from native provider tokens.

use legion_contracts::ids::{
    CheckpointId, CheckpointScopeId, ContextHandoffId, IdError, MessageId, NodeId,
    ProviderDriverKind, ProviderSessionId, ProviderThreadId, ProviderTurnId, RunAttemptId, RunId,
    RuntimeRequestId, ThreadId, TurnItemId,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Encodes an identifier segment using RFC 3986 / JavaScript `encodeURIComponent`
/// semantics so colon-delimited compound IDs stay reversible and URL-safe.
///
/// Unreserved ASCII bytes (`A-Z a-z 0-9 - _ . ! ~ * ' ( )`) remain literal;
/// all other UTF-8 bytes are uppercase `%XX`-escaped.
#[must_use]
pub fn encode_id_component(raw: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(raw.len());
    for byte in raw.as_bytes() {
        let keep = byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
            );
        if keep {
            out.push(char::from(*byte));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(byte >> 4)]));
            out.push(char::from(HEX[usize::from(byte & 0x0F)]));
        }
    }
    out
}

/// Joins a prefix and raw components using `:` with `encodeURIComponent`-style escaping.
#[must_use]
pub fn join_id(prefix: &str, parts: &[&str]) -> String {
    let mut out = String::from(prefix);
    for part in parts {
        out.push(':');
        out.push_str(&encode_id_component(part));
    }
    out
}

/// Returns `true` when `thread_id` represents an internal delegated task or
/// subagent thread (`thread:subagent:...` or `thread:task:...`).
#[must_use]
pub fn is_delegated_task_thread(thread_id: &ThreadId) -> bool {
    let raw = thread_id.as_str();
    raw.starts_with("thread:subagent:") || raw.starts_with("thread:task:")
}

/// Deterministic and random identifier allocator for orchestration and provider runtimes.
///
/// # Examples
///
/// ```
/// use legion_contracts::ids::ProviderDriverKind;
/// use legion_providers::core::IdAllocator;
///
/// let driver = ProviderDriverKind::new("codex")?;
/// let pt = IdAllocator::derive_provider_thread(&driver, "thr/abc 1")?;
/// assert_eq!(pt.as_str(), "pthread:codex:thr%2Fabc%201");
///
/// let sub = IdAllocator::derive_subagent_thread(&pt)?;
/// assert!(IdAllocator::is_subagent_thread(&sub));
/// assert_eq!(sub, IdAllocator::derive_subagent_thread(&pt)?);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct IdAllocator;

impl IdAllocator {
    /// Returns `true` when `thread_id` is a subagent-derived thread identifier.
    #[must_use]
    pub fn is_subagent_thread(thread_id: &ThreadId) -> bool {
        thread_id.as_str().starts_with("thread:subagent:")
    }

    /// Derives a [`ProviderThreadId`] from a driver kind and provider-native thread ID.
    pub fn derive_provider_thread(
        driver: &ProviderDriverKind,
        native_thread_id: &str,
    ) -> Result<ProviderThreadId, IdError> {
        ProviderThreadId::new(join_id("pthread", &[driver.as_str(), native_thread_id]))
    }

    /// Derives a [`ProviderSessionId`] scoped to a thread and provider thread.
    pub fn derive_provider_session(
        thread_id: &ThreadId,
        provider_thread_id: &ProviderThreadId,
    ) -> Result<ProviderSessionId, IdError> {
        ProviderSessionId::new(join_id(
            "psess",
            &[thread_id.as_str(), provider_thread_id.as_str()],
        ))
    }

    /// Derives a collision-resistant subagent [`ThreadId`] using the leading
    /// 16 lowercase hex characters of `SHA-256("thread:<encoded_provider_thread>")`.
    pub fn derive_subagent_thread(
        provider_thread_id: &ProviderThreadId,
    ) -> Result<ThreadId, IdError> {
        let legacy = join_id("thread", &[provider_thread_id.as_str()]);
        let digest = Sha256::digest(legacy.as_bytes());
        let short_hex = &hex::encode(digest)[..16];
        ThreadId::new(format!("thread:subagent:{short_hex}"))
    }

    /// Derives a deterministic [`RunAttemptId`] for `(run_id, attempt_index)`.
    pub fn derive_run_attempt(run_id: &RunId, attempt_index: u32) -> Result<RunAttemptId, IdError> {
        let idx = attempt_index.to_string();
        RunAttemptId::new(join_id("attempt", &[run_id.as_str(), &idx]))
    }

    /// Derives a deterministic [`NodeId`] scoped to `(run_id, key)`.
    pub fn derive_node_id(run_id: &RunId, key: &str) -> Result<NodeId, IdError> {
        NodeId::new(join_id("node", &[run_id.as_str(), key]))
    }

    /// Derives a deterministic [`TurnItemId`] scoped to `(run_id, key)`.
    pub fn derive_turn_item_id(run_id: &RunId, key: &str) -> Result<TurnItemId, IdError> {
        TurnItemId::new(join_id("item", &[run_id.as_str(), key]))
    }

    /// Derives a deterministic [`MessageId`] scoped to `(thread_id, key)`.
    pub fn derive_message_id(thread_id: &ThreadId, key: &str) -> Result<MessageId, IdError> {
        MessageId::new(join_id("msg", &[thread_id.as_str(), key]))
    }

    /// Derives a deterministic [`RuntimeRequestId`] scoped to `(session_id, native_id)`.
    pub fn derive_runtime_request_id(
        session_id: &ProviderSessionId,
        native_request_id: &str,
    ) -> Result<RuntimeRequestId, IdError> {
        RuntimeRequestId::new(join_id("req", &[session_id.as_str(), native_request_id]))
    }

    /// Derives a deterministic [`CheckpointScopeId`] for `(thread_id, workspace_key)`.
    pub fn derive_checkpoint_scope(
        thread_id: &ThreadId,
        workspace_key: &str,
    ) -> Result<CheckpointScopeId, IdError> {
        CheckpointScopeId::new(join_id("cscope", &[thread_id.as_str(), workspace_key]))
    }

    /// Derives a deterministic [`CheckpointId`] for `(scope_id, turn_count)`.
    pub fn derive_checkpoint_id(
        scope_id: &CheckpointScopeId,
        turn_count: u64,
    ) -> Result<CheckpointId, IdError> {
        let count = turn_count.to_string();
        CheckpointId::new(join_id("ckpt", &[scope_id.as_str(), &count]))
    }

    /// Allocates a fresh random [`ThreadId`].
    pub fn allocate_thread_id() -> Result<ThreadId, IdError> {
        ThreadId::new(format!("thread:{}", Uuid::new_v4()))
    }

    /// Allocates a fresh random [`RunId`].
    pub fn allocate_run_id() -> Result<RunId, IdError> {
        RunId::new(format!("run:{}", Uuid::new_v4()))
    }

    /// Allocates a fresh random [`ProviderTurnId`].
    pub fn allocate_provider_turn_id() -> Result<ProviderTurnId, IdError> {
        ProviderTurnId::new(format!("pturn:{}", Uuid::new_v4()))
    }

    /// Allocates a fresh random [`ContextHandoffId`].
    pub fn allocate_context_handoff_id() -> Result<ContextHandoffId, IdError> {
        ContextHandoffId::new(format!("handoff:{}", Uuid::new_v4()))
    }
}
