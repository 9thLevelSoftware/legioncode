//! Pure-Rust host-testable bridge for [`legion_edge::cf_bindings::HookInboxDurableActor`]
//! backing `WasmHookInboxObject`.

use legion_edge::cf_bindings::HookInboxDurableActor;
use legion_edge::relay_hooks::DeliveryOutcome;

use crate::bridge_hook_actor_dto::{
    ActorFetchRequest, ActorSummaryOut, AlarmBatchOut, HoldHookRequest, RecordOutcomeOut,
    RecordOutcomeRequest,
};
use crate::bridge_hook_actor_fetch::{apply_hold, dispatch_actor_fetch};
use crate::bridge_worker_bindings::decode_body_bytes;

/// Host-testable Durable Object webhook inbox actor bridge.
#[derive(Debug, Clone)]
pub struct HookInboxDurableObjectBridge {
    actor: HookInboxDurableActor,
}

impl HookInboxDurableObjectBridge {
    /// Creates an empty [`HookInboxDurableObjectBridge`] for `endpoint_key`.
    pub fn new(endpoint_key: &str) -> Self {
        Self {
            actor: HookInboxDurableActor::new(endpoint_key),
        }
    }

    /// Hydrates a [`HookInboxDurableObjectBridge`] from persisted JSON state.
    pub fn from_state_json(state_json: &str) -> Result<Self, String> {
        let actor: HookInboxDurableActor = serde_json::from_str(state_json)
            .map_err(|err| format!("invalid HookInboxDurableActor JSON: {err}"))?;
        Ok(Self { actor })
    }

    /// Replaces the internal [`HookInboxDurableActor`] from persisted JSON state and returns a
    /// summary JSON snapshot.
    pub fn hydrate_state_json(&mut self, state_json: &str) -> Result<String, String> {
        let actor: HookInboxDurableActor = serde_json::from_str(state_json)
            .map_err(|err| format!("invalid HookInboxDurableActor JSON: {err}"))?;
        self.actor = actor;
        self.summary_json()
    }

    /// Serializes the full [`HookInboxDurableActor`] state to JSON for Durable Object storage.
    pub fn export_state_json(&self) -> Result<String, String> {
        serde_json::to_string(&self.actor)
            .map_err(|err| format!("failed to serialize HookInboxDurableActor: {err}"))
    }

    /// Returns a compact JSON summary of the actor's generation, pending hooks, and alarm time.
    pub fn summary_json(&self) -> Result<String, String> {
        serde_json::to_string(&ActorSummaryOut::from(&self.actor))
            .map_err(|err| format!("failed to serialize actor summary: {err}"))
    }

    /// Borrows the underlying [`HookInboxDurableActor`].
    pub const fn actor(&self) -> &HookInboxDurableActor {
        &self.actor
    }

    /// Holds one webhook request in the inbox and arms the Durable Object alarm.
    pub fn hold_json(&mut self, hook_json: &str) -> Result<String, String> {
        let req: HoldHookRequest = serde_json::from_str(hook_json)
            .map_err(|err| format!("invalid hold hook JSON: {err}"))?;
        let (delivery_id, next_attempt_at_ms) = apply_hold(&mut self.actor, req)?;
        let out = serde_json::json!({
            "held": true,
            "deliveryId": delivery_id,
            "nextAttemptAtMs": next_attempt_at_ms,
            "targetGeneration": self.actor.target_generation(),
            "alarmAtMs": self.actor.alarm_at_ms(),
            "pendingCount": self.actor.pending_count(),
        });
        serde_json::to_string(&out).map_err(|err| format!("failed to serialize hold result: {err}"))
    }

    /// Wakes the actor on `new_base_url`, rotating `target_generation` (`BUG-29`) and arming an
    /// immediate alarm at `now_ms` when hooks are queued.
    pub fn wake(&mut self, new_base_url: &str, now_ms: u64) -> Result<String, String> {
        let pending = self.actor.wake(new_base_url, now_ms);
        let out = serde_json::json!({
            "pending": pending,
            "targetGeneration": self.actor.target_generation(),
            "alarmAtMs": self.actor.alarm_at_ms(),
            "pendingCount": self.actor.pending_count(),
        });
        serde_json::to_string(&out).map_err(|err| format!("failed to serialize wake result: {err}"))
    }

    /// Consumes any armed alarm and captures up to `DELIVERIES_PER_RUN` hooks bound to the active
    /// `target_generation`.
    pub fn begin_alarm_batch(&mut self, now_ms: u64) -> Result<String, String> {
        let batch = self.actor.begin_alarm_batch(now_ms);
        let out = AlarmBatchOut::from_option(batch, &self.actor);
        serde_json::to_string(&out).map_err(|err| format!("failed to serialize alarm batch: {err}"))
    }

    /// Records an upstream delivery outcome, ignoring stale `snapshot_generation` responses (`BUG-29`).
    pub fn record_outcome_json(&mut self, outcome_json: &str) -> Result<String, String> {
        let req: RecordOutcomeRequest = serde_json::from_str(outcome_json)
            .map_err(|err| format!("invalid outcome JSON: {err}"))?;
        let outcome = req.outcome.unwrap_or_else(|| {
            req.status.map_or(
                DeliveryOutcome::Unreachable,
                DeliveryOutcome::from_http_status,
            )
        });
        let result = self.actor.record_delivery_outcome(
            req.snapshot_generation,
            &req.hook_id,
            outcome,
            req.now_ms,
        );
        let out = RecordOutcomeOut::from_result(result, &self.actor);
        serde_json::to_string(&out).map_err(|err| format!("failed to serialize outcome: {err}"))
    }

    /// Finalizes a batch run without `Unreachable` and computes the next alarm timestamp.
    pub fn finish_alarm_batch(
        &mut self,
        delivered: usize,
        busy_hooks_count: usize,
        now_ms: u64,
    ) -> Result<String, String> {
        let next_alarm_at_ms = self
            .actor
            .finish_alarm_batch(delivered, busy_hooks_count, now_ms);
        let out = serde_json::json!({
            "nextAlarmAtMs": next_alarm_at_ms,
            "alarmAtMs": self.actor.alarm_at_ms(),
            "pendingCount": self.actor.pending_count(),
        });
        serde_json::to_string(&out)
            .map_err(|err| format!("failed to serialize finish alarm result: {err}"))
    }

    /// Polls `is_alarm_due(now_ms)` and dispatches `begin_alarm_batch(now_ms)` when due.
    pub fn alarm_step_json(&mut self, now_ms: u64) -> Result<String, String> {
        if !self.actor.is_alarm_due(now_ms) {
            let out = serde_json::json!({
                "due": false,
                "alarmAtMs": self.actor.alarm_at_ms(),
                "pendingCount": self.actor.pending_count(),
                "batch": serde_json::Value::Null,
            });
            return serde_json::to_string(&out)
                .map_err(|err| format!("failed to serialize alarm step: {err}"));
        }
        let batch = self.actor.begin_alarm_batch(now_ms);
        let batch_out = AlarmBatchOut::from_option(batch, &self.actor);
        let out = serde_json::json!({
            "due": true,
            "alarmAtMs": self.actor.alarm_at_ms(),
            "pendingCount": self.actor.pending_count(),
            "batch": batch_out,
        });
        serde_json::to_string(&out).map_err(|err| format!("failed to serialize alarm step: {err}"))
    }

    /// Schedules the `60_000 ms` fallback retry after an unexpected Durable Object alarm exception.
    pub fn record_run_failure(&mut self, now_ms: u64) -> Result<String, String> {
        let retry_at_ms = self.actor.record_run_failure(now_ms);
        let out = serde_json::json!({
            "retryAtMs": retry_at_ms,
            "alarmAtMs": self.actor.alarm_at_ms(),
        });
        serde_json::to_string(&out).map_err(|err| format!("failed to serialize run failure: {err}"))
    }

    /// Dispatches a Durable Object HTTP-style `fetch` envelope (`/hold`, `/wake`, `/deliver-batch`,
    /// `/outcome`, `/finish-batch`, `/run-failure`, `/clear`, `/summary`, `/state`).
    pub fn fetch_json(&mut self, req_json: &str) -> Result<String, String> {
        let req: ActorFetchRequest = serde_json::from_str(req_json)
            .map_err(|err| format!("invalid actor fetch JSON: {err}"))?;
        let path_only = req
            .path
            .split_once('?')
            .map_or(req.path.as_str(), |(p, _)| p);
        let method = req.method.as_deref().map_or_else(
            || {
                if matches!(path_only, "/summary" | "/state") {
                    "GET".to_owned()
                } else {
                    "POST".to_owned()
                }
            },
            str::to_ascii_uppercase,
        );
        let body_bytes = decode_body_bytes(req.body.clone())?;
        let (status, body_val) =
            dispatch_actor_fetch(&mut self.actor, &method, path_only, &req, &body_bytes)?;
        let out = serde_json::json!({
            "status": status,
            "body": body_val,
            "summary": ActorSummaryOut::from(&self.actor),
        });
        serde_json::to_string(&out)
            .map_err(|err| format!("failed to serialize fetch result: {err}"))
    }

    /// Discards all held hooks, target metadata, and pending alarm.
    pub fn clear(&mut self) {
        self.actor.clear();
    }

    /// Returns the scheduled Durable Object alarm timestamp, if any.
    pub const fn alarm_at_ms(&self) -> Option<u64> {
        self.actor.alarm_at_ms()
    }

    /// Returns `true` when an alarm is due at `now_ms`.
    pub const fn is_alarm_due(&self, now_ms: u64) -> bool {
        self.actor.is_alarm_due(now_ms)
    }

    /// Number of queued hooks awaiting delivery.
    pub fn pending_count(&self) -> usize {
        self.actor.pending_count()
    }

    /// Current `target_generation` (`0` before any hook is held).
    pub fn target_generation(&self) -> u64 {
        self.actor.target_generation()
    }
}
