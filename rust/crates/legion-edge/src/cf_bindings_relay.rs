//! Hyperdrive PostgreSQL query plans and serializable Durable Object hook-inbox actor for
//! `infra/relay`.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::relay_hooks::{
    CloudflareApiPauseGate, DeliveryBatch, DeliveryOutcome, DeliveryRecordOutcome, HeldHook,
    HoldRefusalReason, HookInboxState, RUN_FAILURE_RETRY_MS,
};

/// Default `api` key stored in `relay_upstream_api_pauses` (`CloudflareApiPause.ts:9`).
pub const CLOUDFLARE_UPSTREAM_API_KEY: &str = "cloudflare";

/// Postgres table targeted by a [`HyperdriveRelayPlan`] query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HyperdriveTable {
    /// `relay_environment_links` table (`EnvironmentLinks.ts`).
    EnvironmentLinks,
    /// `relay_managed_endpoint_allocations` table (`ManagedEndpointAllocations.ts`).
    ManagedEndpointAllocations,
    /// `relay_upstream_api_pauses` table (`CloudflareApiPause.ts`).
    UpstreamApiPauses,
}

impl HyperdriveTable {
    /// Exact Postgres table name from `infra/relay/src/persistence/schema.ts`.
    pub const fn table_name(self) -> &'static str {
        match self {
            Self::EnvironmentLinks => "relay_environment_links",
            Self::ManagedEndpointAllocations => "relay_managed_endpoint_allocations",
            Self::UpstreamApiPauses => "relay_upstream_api_pauses",
        }
    }
}

/// Parameterized PostgreSQL statement executed over Cloudflare Hyperdrive for `infra/relay`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HyperdriveRelayPlan {
    /// Target PostgreSQL table.
    pub table: HyperdriveTable,
    /// Parameterized PostgreSQL SQL using `$1`, `$2`, ... placeholders.
    pub sql: String,
    /// Ordered positional query parameters.
    pub params: Vec<Value>,
}

impl HyperdriveRelayPlan {
    /// Looks up an active link row in `relay_environment_links` for `(user_id, environment_id)`.
    pub fn get_environment_link(user_id: &str, environment_id: &str) -> Self {
        Self {
            table: HyperdriveTable::EnvironmentLinks,
            sql: "SELECT environment_id, environment_label, environment_public_key, \
                  endpoint_http_base_url, endpoint_ws_base_url, endpoint_provider_kind, \
                  hold_webhooks_while_offline, created_at \
                  FROM relay_environment_links \
                  WHERE user_id = $1 AND environment_id = $2 AND revoked_at IS NULL \
                  LIMIT 1"
                .to_owned(),
            params: vec![json!(user_id), json!(environment_id)],
        }
    }

    /// Finds active managed Cloudflare Tunnel links for `environment_id` (`EnvironmentLinks.ts`).
    pub fn find_active_managed_links(environment_id: &str) -> Self {
        Self {
            table: HyperdriveTable::EnvironmentLinks,
            sql: "SELECT user_id, environment_id, environment_public_key, \
                  endpoint_http_base_url, endpoint_ws_base_url, hold_webhooks_while_offline \
                  FROM relay_environment_links \
                  WHERE environment_id = $1 AND revoked_at IS NULL \
                  AND endpoint_provider_kind = 'cloudflare_tunnel' \
                  AND managed_tunnels_enabled = true"
                .to_owned(),
            params: vec![json!(environment_id)],
        }
    }

    /// Reads an allocation row from `relay_managed_endpoint_allocations`.
    pub fn get_managed_endpoint_allocation(user_id: &str, environment_id: &str) -> Self {
        Self {
            table: HyperdriveTable::ManagedEndpointAllocations,
            sql: "SELECT user_id, environment_id, hostname, tunnel_id, tunnel_name, \
                  dns_record_id, ready_at, origin, updated_at, generation, tunnel_released_at \
                  FROM relay_managed_endpoint_allocations \
                  WHERE user_id = $1 AND environment_id = $2 \
                  LIMIT 1"
                .to_owned(),
            params: vec![json!(user_id), json!(environment_id)],
        }
    }

    /// Atomically claims teardown generation if `(user_id, environment_id, generation)` matches.
    pub fn claim_managed_endpoint_deprovision(
        user_id: &str,
        environment_id: &str,
        generation: u64,
    ) -> Self {
        Self {
            table: HyperdriveTable::ManagedEndpointAllocations,
            sql: "UPDATE relay_managed_endpoint_allocations \
                  SET generation = generation + 1 \
                  WHERE user_id = $1 AND environment_id = $2 AND generation = $3 \
                  RETURNING generation"
                .to_owned(),
            params: vec![json!(user_id), json!(environment_id), json!(generation)],
        }
    }

    /// Reads `paused_until_ms` for `api` from `relay_upstream_api_pauses` (`CloudflareApiPause.ts`).
    pub fn read_upstream_api_pause(api: &str) -> Self {
        Self {
            table: HyperdriveTable::UpstreamApiPauses,
            sql: "SELECT paused_until_ms FROM relay_upstream_api_pauses WHERE api = $1 LIMIT 1"
                .to_owned(),
            params: vec![json!(api)],
        }
    }

    /// Upserts a shared `429` pause floor in `relay_upstream_api_pauses` using `GREATEST`.
    pub fn record_upstream_api_pause(api: &str, paused_until_ms: u64) -> Self {
        Self {
            table: HyperdriveTable::UpstreamApiPauses,
            sql: "INSERT INTO relay_upstream_api_pauses (api, paused_until_ms) VALUES ($1, $2) \
                  ON CONFLICT (api) DO UPDATE \
                  SET paused_until_ms = GREATEST(relay_upstream_api_pauses.paused_until_ms, \
                  EXCLUDED.paused_until_ms)"
                .to_owned(),
            params: vec![json!(api), json!(paused_until_ms)],
        }
    }
}

/// Applies the raw Hyperdrive `relay_upstream_api_pauses` query result to [`CloudflareApiPauseGate`].
///
/// Mirrors `CloudflareApiPause.pausedUntilMillis` (`None => 0`) while keeping the `BUG-32`
/// guarantee intact: transient DB errors (`Err`) leave `gate.checked_at_ms()` unchanged so the
/// `5_000 ms` isolate cache window never suppresses immediate retries.
pub fn apply_upstream_pause_query_result<E>(
    gate: &mut CloudflareApiPauseGate,
    now_ms: u64,
    query_result: Result<Option<u64>, E>,
) -> Result<u64, E> {
    gate.apply_shared_pause_read(now_ms, query_result.map(Option::unwrap_or_default))
}

/// Serializable Durable Object wrapper around [`HookInboxState`] tracking `alarm_at_ms` and
/// `BUG-29` `target_generation` delivery fencing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookInboxDurableActor {
    /// Durable Object key identifying the target environment endpoint.
    pub endpoint_key: String,
    /// Underlying generation-fenced FIFO hook inbox state.
    pub state: HookInboxState,
    /// Scheduled Durable Object alarm timestamp in epoch milliseconds, if armed.
    pub alarm_at_ms: Option<u64>,
}

impl HookInboxDurableActor {
    /// Creates an empty Durable Object actor for `endpoint_key`.
    pub fn new(endpoint_key: impl Into<String>) -> Self {
        Self {
            endpoint_key: endpoint_key.into(),
            state: HookInboxState::new(),
            alarm_at_ms: None,
        }
    }

    /// Borrows the wrapped [`HookInboxState`].
    pub const fn state(&self) -> &HookInboxState {
        &self.state
    }

    /// Returns the currently scheduled Durable Object alarm timestamp, if any.
    pub const fn alarm_at_ms(&self) -> Option<u64> {
        self.alarm_at_ms
    }

    /// Returns the current `target_generation` (`0` before any hook is held).
    pub fn target_generation(&self) -> u64 {
        self.state.target().map_or(0, |t| t.target_generation)
    }

    /// Number of queued hooks awaiting delivery.
    pub fn pending_count(&self) -> usize {
        self.state.pending_count()
    }

    /// Returns `true` when a Durable Object alarm is due at `now_ms`.
    pub const fn is_alarm_due(&self, now_ms: u64) -> bool {
        matches!(self.alarm_at_ms, Some(at) if now_ms >= at)
    }

    /// Holds a webhook request and arms the Durable Object alarm using earliest-wins scheduling.
    pub fn hold(
        &mut self,
        hook: HeldHook,
        base_url: impl Into<String>,
        now_ms: u64,
    ) -> Result<u64, HoldRefusalReason> {
        let next_attempt_at_ms = self.state.hold(hook, base_url, now_ms)?;
        self.schedule_earliest(next_attempt_at_ms);
        Ok(next_attempt_at_ms)
    }

    /// Rotates the active tunnel URL (`BUG-29` `target_generation += 1`, `failures = 0`) and
    /// schedules an immediate alarm at `now_ms` whenever queued hooks exist.
    pub fn wake(&mut self, new_base_url: impl Into<String>, now_ms: u64) -> bool {
        let pending = self.state.wake(new_base_url);
        if pending {
            self.alarm_at_ms = Some(now_ms);
        }
        pending
    }

    /// Clears all held hooks, target metadata, and any pending Durable Object alarm.
    pub fn clear(&mut self) {
        self.state.clear();
        self.alarm_at_ms = None;
    }

    /// Captures up to `DELIVERIES_PER_RUN` held hooks tagged with the active `target_generation`.
    ///
    /// Clears any consumed alarm slot before returning the batch, matching Cloudflare Durable
    /// Object `alarm()` dispatch so a concurrent `wake()` re-arms an immediate run.
    pub fn begin_alarm_batch(&mut self, now_ms: u64) -> Option<DeliveryBatch> {
        self.alarm_at_ms = None;
        self.state.begin_delivery_batch(now_ms)
    }

    /// Records a single upstream delivery result, rejecting stale generation outcomes (`BUG-29`)
    /// and updating `alarm_at_ms` accordingly.
    pub fn record_delivery_outcome(
        &mut self,
        snapshot_generation: u64,
        hook_id: &str,
        outcome: DeliveryOutcome,
        now_ms: u64,
    ) -> DeliveryRecordOutcome {
        let result =
            self.state
                .record_delivery_outcome(snapshot_generation, hook_id, outcome, now_ms);
        match result {
            DeliveryRecordOutcome::Delivered {
                remaining_pending: false,
            } => {
                self.alarm_at_ms = None;
            }
            DeliveryRecordOutcome::Delivered {
                remaining_pending: true,
            }
            | DeliveryRecordOutcome::Busy => {
                self.schedule_earliest(now_ms);
            }
            DeliveryRecordOutcome::Unreachable {
                next_retry_at_ms, ..
            } => {
                self.schedule_earliest(next_retry_at_ms);
            }
            DeliveryRecordOutcome::IgnoredStale {
                next_attempt_at_ms, ..
            } => {
                if let Some(at) = next_attempt_at_ms {
                    self.schedule_earliest(at);
                }
            }
            DeliveryRecordOutcome::IgnoredCleared => {
                if self.state.pending_count() == 0 {
                    self.alarm_at_ms = None;
                }
            }
        }
        result
    }

    /// Finalizes a batch run without `Unreachable` and updates `alarm_at_ms` using earliest-wins.
    pub fn finish_alarm_batch(
        &mut self,
        delivered: usize,
        busy_hooks_count: usize,
        now_ms: u64,
    ) -> Option<u64> {
        let next = self
            .state
            .next_schedule_after_batch(delivered, busy_hooks_count, now_ms);
        next.map(|at| self.schedule_earliest(at))
    }

    /// Schedules the `60_000 ms` fallback retry after an unexpected Durable Object alarm exception.
    pub fn record_run_failure(&mut self, now_ms: u64) -> u64 {
        let retry_at = now_ms.saturating_add(RUN_FAILURE_RETRY_MS);
        self.schedule_earliest(retry_at)
    }

    fn schedule_earliest(&mut self, at_ms: u64) -> u64 {
        let chosen = self.alarm_at_ms.map_or(at_ms, |curr| curr.min(at_ms));
        self.alarm_at_ms = Some(chosen);
        chosen
    }
}
