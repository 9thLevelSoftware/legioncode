//! Durable Object webhook inbox state, generation-fenced delivery outcomes (`BUG-29`),
//! and Cloudflare API rate-limit pause gating (`BUG-32`).
//!
//! Mirrors `infra/relay/src/hooks/{HeldHooks.ts,HookInboxStore.ts,HookInboxObject.ts,upstream.ts}`
//! and `infra/relay/src/environments/{ManagedEndpointProvider.ts,CloudflareApiPause.ts}`.
//!
//! # Examples
//!
//! ```
//! use t3_edge::relay_hooks::{
//!     CloudflareApiPauseGate, DeliveryOutcome, HeldHook, HookInboxState,
//! };
//!
//! let mut inbox = HookInboxState::new();
//! let hook = HeldHook::new("del-1", "hook-1", 1_000, "POST", b"payload".to_vec());
//! assert_eq!(inbox.hold(hook, "https://t1.example.test", 1_000), Ok(11_000));
//!
//! let batch = inbox.begin_delivery_batch(1_000).ok_or("missing batch")?;
//! assert!(inbox.wake("https://t2.example.test"));
//!
//! // Stale 8s upstream failure from generation 1 is ignored against generation 2 (BUG-29).
//! let res = inbox.record_delivery_outcome(
//!     batch.target_generation,
//!     "del-1",
//!     DeliveryOutcome::Unreachable,
//!     9_000,
//! );
//! assert!(res.is_ignored_stale());
//! assert_eq!(inbox.target().map(|t| (t.failures, t.target_generation)), Some((0, 2)));
//!
//! // Cloudflare API pause read error does not advance `checked_at_ms` (BUG-32).
//! let mut gate = CloudflareApiPauseGate::new();
//! let pass = gate.evaluate(1_000, || Err::<u64, _>("read failed"))?;
//! assert_eq!(pass.shared_read_error, Some("read failed"));
//! assert_eq!(gate.checked_at_ms(), None);
//! assert!(gate.should_refresh(1_500));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

pub use crate::relay_hooks_pause::{
    CLOUDFLARE_DEFAULT_RATE_LIMIT_PAUSE_MS, CLOUDFLARE_MAX_RATE_LIMIT_PAUSE_MS,
    CLOUDFLARE_PAUSE_REFRESH_MS, CloudflareApiPauseGate, CloudflareApiPaused, PauseGatePass,
};
pub use crate::relay_hooks_types::{
    BUSY_RETRY_MS, DELIVERIES_PER_RUN, DeliveryOutcome, HOOK_INBOX_MAX_BYTES,
    HOOK_INBOX_MAX_PER_HOOK, HOOK_INBOX_MAX_REQUESTS, HOOK_INBOX_TTL_MS, HeldHook, HeldHooksTarget,
    HoldRefusalReason, MAX_COUNTED_FAILURES, MAX_RETRY_DELAY_MS, RELAY_HOOK_UPSTREAM_TIMEOUT_MS,
    RUN_FAILURE_RETRY_MS, TUNNEL_OFFLINE_STATUS, TargetOutcomeResult, retry_delay_ms,
};

/// Prefetched delivery batch bound to a snapshot `target_generation`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryBatch {
    /// Target generation captured before external HTTP sends start.
    pub target_generation: u64,
    /// Target base URL captured for this batch.
    pub base_url: String,
    /// Up to `DELIVERIES_PER_RUN` held hooks in FIFO order.
    pub hooks: Vec<HeldHook>,
}

/// Result of recording an in-flight delivery outcome against [`HookInboxState`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryRecordOutcome {
    /// Hook was delivered and removed from the inbox.
    Delivered {
        /// Whether more queued hooks remain in the inbox.
        remaining_pending: bool,
    },
    /// Upstream was busy (`429`/`500`); `failures` reset to `0`, hook retained.
    Busy,
    /// Upstream failed/timed out on the active generation; `failures` incremented and retry scheduled.
    Unreachable {
        /// Updated consecutive failure count.
        failures: u32,
        /// Next scheduled retry timestamp in epoch milliseconds.
        next_retry_at_ms: u64,
    },
    /// Snapshot generation did not match `current.target_generation`; stale outcome ignored (`BUG-29`).
    IgnoredStale {
        /// Generation captured when the HTTP send started.
        snapshot_generation: u64,
        /// Current generation after `wake()`.
        current_generation: u64,
        /// Immediate reschedule timestamp if hooks remain queued.
        next_attempt_at_ms: Option<u64>,
    },
    /// Hook or target was cleared while the upstream send was in flight; outcome ignored.
    IgnoredCleared,
}

impl DeliveryRecordOutcome {
    /// Returns `true` when the outcome was ignored due to a stale `target_generation`.
    pub fn is_ignored_stale(self) -> bool {
        matches!(self, Self::IgnoredStale { .. })
    }
}

/// Generation-fenced Durable Object hook inbox state (`BUG-29`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookInboxState {
    hooks: VecDeque<HeldHook>,
    total_bytes: usize,
    target: Option<HeldHooksTarget>,
    last_generation: u64,
}

impl HookInboxState {
    /// Creates an empty hook inbox state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the active delivery target, if any hooks have been held.
    pub fn target(&self) -> Option<&HeldHooksTarget> {
        self.target.as_ref()
    }

    /// Number of currently held webhook requests.
    pub fn pending_count(&self) -> usize {
        self.hooks.len()
    }

    /// Total body bytes across all currently held webhook requests.
    pub fn pending_bytes(&self) -> usize {
        self.total_bytes
    }

    /// Returns an iterator over held hooks in FIFO order.
    pub fn pending_hooks(&self) -> impl Iterator<Item = &HeldHook> {
        self.hooks.iter()
    }

    /// Holds a webhook request for `base_url` unless inbox limits or duplicate ID refuse it.
    ///
    /// Returns the first attempt timestamp (`now_ms + retry_delay_ms(failures)`).
    pub fn hold(
        &mut self,
        hook: HeldHook,
        base_url: impl Into<String>,
        now_ms: u64,
    ) -> Result<u64, HoldRefusalReason> {
        if self.hooks.iter().any(|existing| existing.id == hook.id) {
            return Err(HoldRefusalReason::AlreadyHeld);
        }
        if self.hooks.len() >= HOOK_INBOX_MAX_REQUESTS {
            return Err(HoldRefusalReason::MaxRequests);
        }
        let per_hook = self
            .hooks
            .iter()
            .filter(|h| h.hook_key == hook.hook_key)
            .count();
        if per_hook >= HOOK_INBOX_MAX_PER_HOOK {
            return Err(HoldRefusalReason::MaxPerHook);
        }
        if self.total_bytes.saturating_add(hook.body.len()) > HOOK_INBOX_MAX_BYTES {
            return Err(HoldRefusalReason::MaxBytes);
        }

        let next_url = base_url.into();
        self.total_bytes = self.total_bytes.saturating_add(hook.body.len());
        self.hooks.push_back(hook);

        let failures = if let Some(target) = self.target.as_mut() {
            if target.base_url != next_url {
                target.base_url = next_url;
                target.target_generation = target.target_generation.saturating_add(1);
                self.last_generation = self.last_generation.max(target.target_generation);
            }
            target.failures
        } else {
            self.last_generation = self.last_generation.saturating_add(1);
            self.target = Some(HeldHooksTarget::with_generation(
                next_url,
                self.last_generation,
            ));
            0
        };
        Ok(now_ms.saturating_add(retry_delay_ms(failures)))
    }

    /// Wakes the inbox on `new_base_url`, incrementing `target_generation` and resetting `failures = 0`.
    ///
    /// Returns `true` when pending hooks exist and an immediate delivery alarm should be scheduled.
    pub fn wake(&mut self, new_base_url: impl Into<String>) -> bool {
        let url = new_base_url.into();
        if let Some(target) = self.target.as_mut() {
            let gen_id = target.wake(url);
            self.last_generation = self.last_generation.max(gen_id);
        } else if !self.hooks.is_empty() {
            self.last_generation = self.last_generation.saturating_add(1);
            self.target = Some(HeldHooksTarget::with_generation(url, self.last_generation));
        }
        !self.hooks.is_empty()
    }

    /// Discards all held hooks and clears the active target metadata.
    pub fn clear(&mut self) {
        self.hooks.clear();
        self.total_bytes = 0;
        self.target = None;
    }

    /// Removes hooks older than `HOOK_INBOX_TTL_MS` (`24 hours`) and returns the number expired.
    pub fn prune_expired(&mut self, now_ms: u64) -> usize {
        let mut expired = 0usize;
        self.hooks.retain(|hook| {
            let keep = now_ms <= hook.received_at_ms.saturating_add(HOOK_INBOX_TTL_MS);
            if !keep {
                expired += 1;
                self.total_bytes = self.total_bytes.saturating_sub(hook.body.len());
            }
            keep
        });
        expired
    }

    /// Expires stale hooks and captures a snapshot batch bound to `target_generation`.
    pub fn begin_delivery_batch(&mut self, now_ms: u64) -> Option<DeliveryBatch> {
        self.prune_expired(now_ms);
        let Some(target) = self.target.as_mut() else {
            self.hooks.clear();
            self.total_bytes = 0;
            return None;
        };
        if self.hooks.is_empty() {
            target.failures = 0;
            return None;
        }
        let hooks: Vec<HeldHook> = self
            .hooks
            .iter()
            .take(DELIVERIES_PER_RUN)
            .cloned()
            .collect();
        Some(DeliveryBatch {
            target_generation: target.target_generation,
            base_url: target.base_url.clone(),
            hooks,
        })
    }

    /// Records the outcome of an upstream send started against `snapshot_generation` (`BUG-29`).
    ///
    /// If `wake()` rotated `target_generation` while the 8s upstream send was in flight, any
    /// stale outcome (including `Unreachable`) is ignored so `failures` stays `0` on the new
    /// target and the hook remains queued for immediate delivery to the new URL.
    pub fn record_delivery_outcome(
        &mut self,
        snapshot_generation: u64,
        hook_id: &str,
        outcome: DeliveryOutcome,
        now_ms: u64,
    ) -> DeliveryRecordOutcome {
        let Some(target) = self.target.as_mut() else {
            return DeliveryRecordOutcome::IgnoredCleared;
        };
        let Some(pos) = self.hooks.iter().position(|hook| hook.id == hook_id) else {
            return DeliveryRecordOutcome::IgnoredCleared;
        };
        match target.record_outcome(snapshot_generation, outcome) {
            TargetOutcomeResult::IgnoredStale { current_generation } => {
                DeliveryRecordOutcome::IgnoredStale {
                    snapshot_generation,
                    current_generation,
                    next_attempt_at_ms: (!self.hooks.is_empty()).then_some(now_ms),
                }
            }
            TargetOutcomeResult::Applied { failures } => match outcome {
                DeliveryOutcome::Unreachable => DeliveryRecordOutcome::Unreachable {
                    failures,
                    next_retry_at_ms: now_ms.saturating_add(retry_delay_ms(failures)),
                },
                DeliveryOutcome::Busy => DeliveryRecordOutcome::Busy,
                DeliveryOutcome::Delivered => {
                    if let Some(removed) = self.hooks.remove(pos) {
                        self.total_bytes = self.total_bytes.saturating_sub(removed.body.len());
                    }
                    DeliveryRecordOutcome::Delivered {
                        remaining_pending: !self.hooks.is_empty(),
                    }
                }
            },
        }
    }

    /// Computes the next alarm time after a batch that finished without hitting `Unreachable`.
    pub fn next_schedule_after_batch(
        &self,
        delivered: usize,
        busy_hooks_count: usize,
        now_ms: u64,
    ) -> Option<u64> {
        if self.hooks.is_empty() {
            return None;
        }
        let idle = delivered == 0 && busy_hooks_count > 0;
        Some(if idle {
            now_ms.saturating_add(BUSY_RETRY_MS)
        } else {
            now_ms
        })
    }
}
