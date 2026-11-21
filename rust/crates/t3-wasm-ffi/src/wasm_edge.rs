//! `#[wasm_bindgen]` Cloudflare Worker router and Durable Object actor exports.

use wasm_bindgen::prelude::{JsValue, wasm_bindgen};

use crate::bridge;

/// Stateful Cloudflare Worker router exported to JavaScript / Worker callers.
#[wasm_bindgen]
pub struct WasmEdgeWorker {
    inner: bridge::EdgeWorkerBridge,
}

impl Default for WasmEdgeWorker {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl WasmEdgeWorker {
    /// Creates a new [`WasmEdgeWorker`] backed by [`t3_surfaces::cf_worker::CloudflareWorkerRouter`].
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            inner: bridge::EdgeWorkerBridge::new(),
        }
    }

    /// Dispatches one JSON HTTP request envelope through [`t3_surfaces::cf_worker::CloudflareWorkerRouter`].
    #[wasm_bindgen]
    pub fn handle_request_json(&mut self, req_json: &str) -> Result<String, JsValue> {
        self.inner
            .handle_request_json(req_json)
            .map_err(|err| JsValue::from_str(&err))
    }

    /// Dispatches one JSON HTTP request envelope with optional preloaded Cloudflare binding reads
    /// (`preloaded`) and returns `{ "response": ..., "effects": [...] }`.
    #[wasm_bindgen]
    pub fn dispatch_with_bindings_json(&mut self, req_json: &str) -> Result<String, JsValue> {
        self.inner
            .dispatch_with_bindings_json(req_json)
            .map_err(|err| JsValue::from_str(&err))
    }

    /// Evaluates a scheduled Cloudflare Worker cron trigger (`"* * * * *"` or `"0 * * * *"`)
    /// at explicit JavaScript timestamp `now_ms`.
    #[wasm_bindgen]
    pub fn handle_scheduled_json(&mut self, cron: &str, now_ms: f64) -> Result<String, JsValue> {
        let ts = bridge::js_ms_to_u64(now_ms).map_err(|err| JsValue::from_str(&err))?;
        self.inner
            .handle_scheduled_json(cron, ts)
            .map_err(|err| JsValue::from_str(&err))
    }
}

/// Stateful Durable Object webhook inbox actor exported to Cloudflare Worker callers.
#[wasm_bindgen]
pub struct WasmHookInboxObject {
    inner: bridge::HookInboxDurableObjectBridge,
}

#[wasm_bindgen]
impl WasmHookInboxObject {
    /// Creates an empty [`WasmHookInboxObject`] for `endpoint_key`.
    #[wasm_bindgen(constructor)]
    pub fn new(endpoint_key: &str) -> Self {
        Self {
            inner: bridge::HookInboxDurableObjectBridge::new(endpoint_key),
        }
    }

    /// Hydrates a new [`WasmHookInboxObject`] from persisted JSON state.
    #[wasm_bindgen]
    pub fn from_state_json(state_json: &str) -> Result<Self, JsValue> {
        let inner = bridge::HookInboxDurableObjectBridge::from_state_json(state_json)
            .map_err(|err| JsValue::from_str(&err))?;
        Ok(Self { inner })
    }

    /// Replaces internal state from persisted JSON and returns a summary JSON snapshot.
    #[wasm_bindgen]
    pub fn hydrate_state_json(&mut self, state_json: &str) -> Result<String, JsValue> {
        self.inner
            .hydrate_state_json(state_json)
            .map_err(|err| JsValue::from_str(&err))
    }

    /// Exports full [`t3_edge::cf_bindings::HookInboxDurableActor`] JSON state for storage.
    #[wasm_bindgen]
    pub fn export_state_json(&self) -> Result<String, JsValue> {
        self.inner
            .export_state_json()
            .map_err(|err| JsValue::from_str(&err))
    }

    /// Returns a compact JSON summary of generation, pending hooks, and scheduled alarm time.
    #[wasm_bindgen]
    pub fn summary_json(&self) -> Result<String, JsValue> {
        self.inner
            .summary_json()
            .map_err(|err| JsValue::from_str(&err))
    }

    /// Dispatches a Durable Object HTTP-style `fetch` envelope (`/hold`, `/wake`, `/deliver-batch`,
    /// `/outcome`, `/finish-batch`, `/run-failure`, `/clear`, `/summary`, `/state`).
    #[wasm_bindgen]
    pub fn fetch_json(&mut self, req_json: &str) -> Result<String, JsValue> {
        self.inner
            .fetch_json(req_json)
            .map_err(|err| JsValue::from_str(&err))
    }

    /// Holds one webhook request JSON payload and arms the Durable Object alarm.
    #[wasm_bindgen]
    pub fn hold_json(&mut self, hook_json: &str) -> Result<String, JsValue> {
        self.inner
            .hold_json(hook_json)
            .map_err(|err| JsValue::from_str(&err))
    }

    /// Wakes the actor on `new_base_url`, rotating `target_generation` (`BUG-29`) and scheduling
    /// an immediate alarm at `now_ms` when hooks are queued.
    #[wasm_bindgen]
    pub fn wake(&mut self, new_base_url: &str, now_ms: f64) -> Result<String, JsValue> {
        let ts = bridge::js_ms_to_u64(now_ms).map_err(|err| JsValue::from_str(&err))?;
        self.inner
            .wake(new_base_url, ts)
            .map_err(|err| JsValue::from_str(&err))
    }

    /// Consumes any armed alarm and captures up to `DELIVERIES_PER_RUN` hooks for delivery.
    #[wasm_bindgen]
    pub fn begin_alarm_batch(&mut self, now_ms: f64) -> Result<String, JsValue> {
        let ts = bridge::js_ms_to_u64(now_ms).map_err(|err| JsValue::from_str(&err))?;
        self.inner
            .begin_alarm_batch(ts)
            .map_err(|err| JsValue::from_str(&err))
    }

    /// Records an upstream delivery outcome JSON payload, ignoring stale generations (`BUG-29`).
    #[wasm_bindgen]
    pub fn record_outcome_json(&mut self, outcome_json: &str) -> Result<String, JsValue> {
        self.inner
            .record_outcome_json(outcome_json)
            .map_err(|err| JsValue::from_str(&err))
    }

    /// Finalizes a batch run without `Unreachable` and returns the next scheduled alarm time.
    #[wasm_bindgen]
    pub fn finish_alarm_batch(
        &mut self,
        delivered: u32,
        busy_hooks_count: u32,
        now_ms: f64,
    ) -> Result<String, JsValue> {
        let ts = bridge::js_ms_to_u64(now_ms).map_err(|err| JsValue::from_str(&err))?;
        self.inner
            .finish_alarm_batch(delivered as usize, busy_hooks_count as usize, ts)
            .map_err(|err| JsValue::from_str(&err))
    }

    /// Polls `is_alarm_due(now_ms)` and dispatches an alarm batch when due.
    #[wasm_bindgen]
    pub fn alarm_step_json(&mut self, now_ms: f64) -> Result<String, JsValue> {
        let ts = bridge::js_ms_to_u64(now_ms).map_err(|err| JsValue::from_str(&err))?;
        self.inner
            .alarm_step_json(ts)
            .map_err(|err| JsValue::from_str(&err))
    }

    /// Schedules the `60_000 ms` fallback retry after an unexpected Durable Object alarm exception.
    #[wasm_bindgen]
    pub fn record_run_failure(&mut self, now_ms: f64) -> Result<String, JsValue> {
        let ts = bridge::js_ms_to_u64(now_ms).map_err(|err| JsValue::from_str(&err))?;
        self.inner
            .record_run_failure(ts)
            .map_err(|err| JsValue::from_str(&err))
    }

    /// Discards all held hooks, target metadata, and pending alarm.
    #[wasm_bindgen]
    pub fn clear(&mut self) {
        self.inner.clear();
    }

    /// Returns the scheduled Durable Object alarm timestamp in epoch milliseconds, if armed.
    #[wasm_bindgen]
    pub fn alarm_at_ms(&self) -> Option<f64> {
        self.inner.alarm_at_ms().map(bridge::u64_to_f64_exact)
    }

    /// Returns `true` when an alarm is due at `now_ms`.
    #[wasm_bindgen]
    pub fn is_alarm_due(&self, now_ms: f64) -> Result<bool, JsValue> {
        let ts = bridge::js_ms_to_u64(now_ms).map_err(|err| JsValue::from_str(&err))?;
        Ok(self.inner.is_alarm_due(ts))
    }

    /// Returns the number of queued webhooks awaiting delivery.
    #[wasm_bindgen]
    pub fn pending_count(&self) -> u32 {
        u32::try_from(self.inner.pending_count()).unwrap_or(u32::MAX)
    }

    /// Returns the current monotonic `target_generation` (`0` before any hook is held).
    #[wasm_bindgen]
    pub fn target_generation(&self) -> f64 {
        bridge::u64_to_f64_exact(self.inner.target_generation())
    }
}
