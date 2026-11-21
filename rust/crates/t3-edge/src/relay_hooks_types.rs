//! Core webhook inbox models, retry schedule, and generation-fenced target (`BUG-29`).

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Maximum age of held webhooks before expiry (`24 hours`).
pub const HOOK_INBOX_TTL_MS: u64 = 24 * 60 * 60 * 1_000;

/// Maximum total queued webhook requests in a single Durable Object inbox.
pub const HOOK_INBOX_MAX_REQUESTS: usize = 1_000;

/// Maximum total queued webhook body bytes (`50 MiB`).
pub const HOOK_INBOX_MAX_BYTES: usize = 50 * 1_048_576;

/// Maximum queued requests per individual `hook_key`.
pub const HOOK_INBOX_MAX_PER_HOOK: usize = 100;

/// Maximum deliveries dispatched in one alarm run (`HookInboxStore.ts:19`).
pub const DELIVERIES_PER_RUN: usize = 20;

/// Delay before retrying when every pending hook returned `429`/`500` busy.
pub const BUSY_RETRY_MS: u64 = 30_000;

/// Maximum exponential retry delay (`10 minutes`).
pub const MAX_RETRY_DELAY_MS: u64 = 10 * 60 * 1_000;

/// Cap on consecutive failures counted toward retry backoff (`HookInboxStore.ts:26`).
pub const MAX_COUNTED_FAILURES: u32 = 25;

/// Upstream webhook forwarding timeout (`upstream.ts:17`).
pub const RELAY_HOOK_UPSTREAM_TIMEOUT_MS: u64 = 8_000;

/// Cloudflare status returned when a tunnel has no connected origin (`upstream.ts:19`).
pub const TUNNEL_OFFLINE_STATUS: u16 = 530;

/// Alarm retry delay when an unexpected inbox delivery run error occurs (`HookInboxObject.ts:16`).
pub const RUN_FAILURE_RETRY_MS: u64 = 60_000;

/// Computes the retry backoff in milliseconds for `failures` consecutive unreachable attempts.
///
/// Retries every `10_000 ms` for the first 18 failures (`3 minutes` total) before switching to
/// exponential backoff starting at `30_000 ms`, capped at `MAX_RETRY_DELAY_MS` (`600_000 ms`).
pub fn retry_delay_ms(failures: u32) -> u64 {
    let clamped = failures.clamp(1, MAX_COUNTED_FAILURES);
    if clamped <= 18 {
        return 10_000;
    }
    let exp = clamped - 19;
    30_000_u64
        .saturating_mul(1_u64 << exp)
        .min(MAX_RETRY_DELAY_MS)
}

/// Held webhook payload stored byte-for-byte in the Durable Object inbox (`HookInboxStore.ts:40`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldHook {
    /// Unique delivery identifier used for environment deduplication.
    pub id: String,
    /// Epoch milliseconds when the relay received the webhook.
    pub received_at_ms: u64,
    /// HTTP method (`POST`, `PUT`, etc.).
    pub method: String,
    /// Percent-encoded hook ID segment as received.
    pub raw_hook_id: String,
    /// Percent-encoded hook token segment as received.
    pub raw_token: String,
    /// Decoded hook key used to isolate per-hook caps.
    pub hook_key: String,
    /// Query string without leading `?`.
    pub query: String,
    /// Forwarded HTTP headers.
    pub headers: IndexMap<String, String>,
    /// Raw HTTP request body bytes.
    pub body: Vec<u8>,
}

impl HeldHook {
    /// Constructs a minimal [`HeldHook`] with default raw path fields derived from `hook_key`.
    pub fn new(
        id: impl Into<String>,
        hook_key: impl Into<String>,
        received_at_ms: u64,
        method: impl Into<String>,
        body: Vec<u8>,
    ) -> Self {
        let key = hook_key.into();
        Self {
            id: id.into(),
            received_at_ms,
            method: method.into(),
            raw_hook_id: key.clone(),
            raw_token: String::new(),
            hook_key: key,
            query: String::new(),
            headers: IndexMap::new(),
            body,
        }
    }
}

/// Classification of an upstream webhook send attempt (`HookInboxStore.ts:55`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryOutcome {
    /// Environment accepted or handled the webhook (removed from inbox).
    Delivered,
    /// Environment is busy (`429` or `500`); keep hook and try other hooks.
    Busy,
    /// Tunnel/origin is unreachable (`502`, `503`, `504`, `530`, transport error, or 8s timeout).
    Unreachable,
}

impl DeliveryOutcome {
    /// Maps an upstream HTTP status code according to `HookInboxObject.ts:13-15,49-53`.
    pub fn from_http_status(status: u16) -> Self {
        match status {
            502 | 503 | 504 | TUNNEL_OFFLINE_STATUS => Self::Unreachable,
            429 | 500 => Self::Busy,
            _ => Self::Delivered,
        }
    }

    /// Maps an upstream send `Result<status, err>` (including timeouts as `Err`) to [`DeliveryOutcome`].
    pub fn from_send_result<E>(result: &Result<u16, E>) -> Self {
        match result {
            Ok(status) => Self::from_http_status(*status),
            Err(_) => Self::Unreachable,
        }
    }
}

/// Reason a webhook could not be stored in `HookInboxState::hold`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Error)]
#[serde(rename_all = "snake_case")]
pub enum HoldRefusalReason {
    /// Delivery ID is already present in the inbox.
    #[error("webhook delivery is already held")]
    AlreadyHeld,
    /// Inbox already holds `HOOK_INBOX_MAX_REQUESTS` (`1_000`) items.
    #[error("hook inbox reached maximum request capacity")]
    MaxRequests,
    /// Adding this body would exceed `HOOK_INBOX_MAX_BYTES` (`50 MiB`).
    #[error("hook inbox reached maximum byte capacity")]
    MaxBytes,
    /// This `hook_key` already holds `HOOK_INBOX_MAX_PER_HOOK` (`100`) items.
    #[error("hook key reached per-hook queue limit")]
    MaxPerHook,
}

/// Delivery target metadata carrying a monotonic `target_generation` (`BUG-29`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldHooksTarget {
    /// Current environment tunnel base URL.
    pub base_url: String,
    /// Consecutive unreachable failures recorded against the current generation.
    pub failures: u32,
    /// Monotonically incremented each time `wake(new_base_url)` or URL rotation replaces the target.
    pub target_generation: u64,
}

impl HeldHooksTarget {
    /// Creates a new target at generation `1` with zero failures.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self::with_generation(base_url, 1)
    }

    /// Creates a new target with an explicit starting `target_generation` and zero failures.
    pub fn with_generation(base_url: impl Into<String>, target_generation: u64) -> Self {
        Self {
            base_url: base_url.into(),
            failures: 0,
            target_generation,
        }
    }

    /// Wakes the target on `new_base_url`, incrementing `target_generation` and resetting `failures = 0`.
    pub fn wake(&mut self, new_base_url: impl Into<String>) -> u64 {
        self.base_url = new_base_url.into();
        self.failures = 0;
        self.target_generation = self.target_generation.saturating_add(1);
        self.target_generation
    }

    /// Applies an upstream delivery outcome only if `snapshot_generation == self.target_generation`.
    pub fn record_outcome(
        &mut self,
        snapshot_generation: u64,
        outcome: DeliveryOutcome,
    ) -> TargetOutcomeResult {
        if snapshot_generation != self.target_generation {
            return TargetOutcomeResult::IgnoredStale {
                current_generation: self.target_generation,
            };
        }
        match outcome {
            DeliveryOutcome::Delivered | DeliveryOutcome::Busy => {
                self.failures = 0;
            }
            DeliveryOutcome::Unreachable => {
                self.failures = self.failures.saturating_add(1).min(MAX_COUNTED_FAILURES);
            }
        }
        TargetOutcomeResult::Applied {
            failures: self.failures,
        }
    }
}

/// Direct result of applying an outcome to [`HeldHooksTarget`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetOutcomeResult {
    /// Outcome matched `target_generation` and updated `failures`.
    Applied {
        /// Consecutive failures after applying the outcome.
        failures: u32,
    },
    /// Outcome carried an outdated `snapshot_generation` and was ignored.
    IgnoredStale {
        /// Active `target_generation` on the target.
        current_generation: u64,
    },
}
