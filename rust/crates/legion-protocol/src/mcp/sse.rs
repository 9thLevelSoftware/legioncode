//! Wait-timeout clamping, multi-event SSE JSON-RPC parsing, and single-flight peer init.
//!
//! Fixes BUG-06 in `apps/server/src/peer/PeerMcpClient.ts` and enforces the
//! bounded `waitForTurn` ceiling from `apps/server/src/mcp/OrchestratorMcpService.ts`.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::sync::{Arc, PoisonError, RwLock};

use legion_contracts::ids::EnvironmentId;
use legion_shared::concurrency::KeyedLock;
use serde_json::Value;
use thiserror::Error;

use crate::rpc_codec::RpcId;

/// Default `waitForTurn` timeout in milliseconds when omitted by callers.
pub const DEFAULT_WAIT_TIMEOUT_MS: u64 = 30_000;

/// Maximum `waitForTurn` timeout in milliseconds allowed on `/mcp`.
pub const MAX_WAIT_TIMEOUT_MS: u64 = 45_000;

/// Clamps an optional `waitForTurn` timeout into `[1, MAX_WAIT_TIMEOUT_MS]`, defaulting to `30_000`.
pub fn clamp_wait_timeout_ms(requested_ms: Option<i64>) -> u64 {
    match requested_ms {
        None => DEFAULT_WAIT_TIMEOUT_MS,
        Some(ms) if ms <= 1 => 1,
        Some(ms) => ms.cast_unsigned().min(MAX_WAIT_TIMEOUT_MS),
    }
}

/// Failure raised while parsing an SSE or JSON-RPC `/mcp` response payload.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum McpSseError {
    /// An SSE event `data:` block contained invalid JSON.
    #[error("invalid JSON in SSE event block #{event_index}: {message}")]
    InvalidEventJson {
        /// Zero-based index of the malformed event block.
        event_index: usize,
        /// JSON parser message.
        message: String,
    },
    /// Response contained no JSON-RPC payload matching the expected `id`.
    #[error("no JSON-RPC response matched id {expected_id}")]
    MissingMatchingResponse {
        /// Request identifier that was awaited.
        expected_id: RpcId,
    },
}

fn json_id_matches(candidate: Option<&Value>, expected: &RpcId) -> bool {
    match (candidate, expected) {
        (Some(Value::String(actual)), RpcId::String(exp)) => actual == exp,
        (Some(Value::Number(actual)), RpcId::Integer(exp)) => actual.as_i64() == Some(*exp),
        _ => false,
    }
}

/// Parses an SSE response body by splitting `\n\n` event blocks and returning the JSON-RPC
/// object whose `"id"` matches `expected_id`.
///
/// Server notifications (events missing `"id"` or targeting other IDs) and comment lines
/// are skipped cleanly rather than concatenated into corrupted JSON (BUG-06).
pub fn parse_sse_json_rpc_response(
    sse_body: &str,
    expected_id: &RpcId,
) -> Result<Value, McpSseError> {
    let normalized = sse_body.replace("\r\n", "\n").replace('\r', "\n");
    for (event_index, block) in normalized.split("\n\n").enumerate() {
        let mut data_lines = Vec::new();
        for line in block.lines() {
            if let Some(after_prefix) = line.strip_prefix("data:") {
                let cleaned = after_prefix.strip_prefix(' ').unwrap_or(after_prefix);
                data_lines.push(cleaned);
            }
        }
        if data_lines.is_empty() {
            continue;
        }
        let payload = data_lines.join("\n");
        let trimmed = payload.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parsed: Value =
            serde_json::from_str(trimmed).map_err(|err| McpSseError::InvalidEventJson {
                event_index,
                message: err.to_string(),
            })?;
        if json_id_matches(parsed.get("id"), expected_id) {
            return Ok(parsed);
        }
    }

    Err(McpSseError::MissingMatchingResponse {
        expected_id: expected_id.clone(),
    })
}

/// Cache key identifying a remote peer MCP session.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PeerSessionKey {
    /// Target environment identifier.
    pub environment_id: EnvironmentId,
    /// Target MCP endpoint URL.
    pub endpoint_url: String,
}

impl PeerSessionKey {
    /// Builds a [`PeerSessionKey`] for `(environment_id, endpoint_url)`.
    pub fn new(environment_id: EnvironmentId, endpoint_url: impl Into<String>) -> Self {
        Self {
            environment_id,
            endpoint_url: endpoint_url.into(),
        }
    }
}

/// Established remote peer MCP session metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerMcpSessionState {
    /// Negotiated `Mcp-Session-Id` header value.
    pub session_id: String,
    /// Negotiated MCP protocol version, if advertised.
    pub protocol_version: Option<String>,
    /// Optional server info object returned by `initialize`.
    pub server_info: Option<Value>,
}

/// Single-flight session initialization map for outbound peer `/mcp` connections.
///
/// Serializes concurrent `initialize` handshakes per [`PeerSessionKey`] via
/// [`KeyedLock`] so parallel callers reuse one initialized session instead of
/// launching duplicate `initialize` requests (BUG-06).
#[derive(Clone, Default)]
pub struct PeerMcpSessionMap {
    sessions: Arc<RwLock<HashMap<PeerSessionKey, PeerMcpSessionState>>>,
    init_locks: KeyedLock<PeerSessionKey>,
}

impl fmt::Debug for PeerMcpSessionMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PeerMcpSessionMap")
            .field("session_count", &self.len())
            .finish_non_exhaustive()
    }
}

impl PeerMcpSessionMap {
    /// Creates an empty [`PeerMcpSessionMap`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a cloned snapshot of an already-initialized peer session, if present.
    pub fn get(&self, key: &PeerSessionKey) -> Option<PeerMcpSessionState> {
        self.sessions
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
            .cloned()
    }

    /// Returns an existing peer session or runs `initializer` at most once concurrently per key.
    pub async fn get_or_init<F, Fut, E>(
        &self,
        key: PeerSessionKey,
        initializer: F,
    ) -> Result<PeerMcpSessionState, E>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<PeerMcpSessionState, E>>,
    {
        if let Some(existing) = self.get(&key) {
            return Ok(existing);
        }

        self.init_locks
            .with_lock(key.clone(), async {
                if let Some(existing) = self.get(&key) {
                    return Ok(existing);
                }
                let session_state = initializer().await?;
                self.sessions
                    .write()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(key, session_state.clone());
                Ok(session_state)
            })
            .await
    }

    /// Removes the session entry only when its `session_id` matches `expected_session_id`.
    pub fn invalidate_if_matches(&self, key: &PeerSessionKey, expected_session_id: &str) -> bool {
        let mut guard = self
            .sessions
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        let matches = guard
            .get(key)
            .is_some_and(|entry| entry.session_id == expected_session_id);
        if matches {
            guard.remove(key);
        }
        matches
    }

    /// Removes all peer sessions targeting `environment_id`.
    pub fn invalidate_environment(&self, environment_id: &EnvironmentId) {
        self.sessions
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|key, _| &key.environment_id != environment_id);
    }

    /// Returns the number of cached peer MCP sessions.
    pub fn len(&self) -> usize {
        self.sessions
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// Returns `true` when no peer sessions are cached.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
