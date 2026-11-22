//! `wasm32`-clean Cloudflare Worker HTTP dispatcher wiring [`legion_edge::relay_hooks`],
//! [`legion_edge::task_cache`], and [`legion_edge::cf_bindings`] behind typed route handlers.

#[path = "cf_worker_bindings.rs"]
pub(crate) mod cf_worker_bindings;
#[path = "cf_worker_native.rs"]
mod cf_worker_native;
#[path = "cf_worker_pause.rs"]
mod cf_worker_pause;

#[cfg(test)]
#[path = "cf_worker_tests.rs"]
mod tests;

use std::collections::BTreeMap;

use legion_edge::cf_bindings::{HookInboxDurableActor, MAX_GC_BATCH_SIZE, run_scheduled_cron};
use legion_edge::relay_hooks::{CloudflareApiPauseGate, HookInboxState};
use legion_edge::task_cache::{CacheAdmissionPolicy, D1BatchLedger, OidcCooldownTracker};
use serde::{Deserialize, Serialize};

use crate::cf_worker::cf_worker_native::handle_native_clients;
use crate::cf_worker::cf_worker_pause::handle_pause_gate;
use crate::cf_worker_cache::handle_task_cache;
use crate::cf_worker_hooks::{handle_internal_hook, handle_public_hook};

pub use crate::cf_worker::cf_worker_bindings::{
    BindingReadResults, EdgeHostEffect, PreloadedCacheHit, WorkerDispatchOutcome,
};
pub use crate::cf_worker::cf_worker_pause::{PauseEvaluatePayload, RateLimitPayload};
pub use crate::cf_worker_cache::{CacheFetchPayload, CacheStorePayload};
pub use crate::cf_worker_hooks::{HookOutcomePayload, WakePayload};
pub use legion_edge::cf_bindings::ScheduledCronReport;

/// Platform-agnostic HTTP request handed to [`CloudflareWorkerRouter::dispatch`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerHttpRequest {
    /// Uppercase HTTP method (`"GET"`, `"POST"`, `"PUT"`, ...).
    pub method: String,
    /// Request path without scheme/host (e.g. `"/v1/hooks/env/h1/tok"`).
    pub path: String,
    /// Optional raw query string without leading `'?'`.
    pub query: Option<String>,
    /// Case-insensitive HTTP request headers.
    pub headers: BTreeMap<String, String>,
    /// Raw request payload bytes.
    pub body: Vec<u8>,
    /// Explicit deterministic wall-clock timestamp in epoch milliseconds (`wasm32`-safe).
    pub now_ms: u64,
}

impl WorkerHttpRequest {
    /// Builds a request for `method` + `path` at explicit `now_ms`.
    pub fn new(method: impl Into<String>, path: impl Into<String>, now_ms: u64) -> Self {
        let raw_path = path.into();
        let (path_only, query) = match raw_path.split_once('?') {
            Some((p, q)) => (p.to_owned(), Some(q.to_owned())),
            None => (raw_path, None),
        };
        Self {
            method: method.into().to_ascii_uppercase(),
            path: path_only,
            query,
            headers: BTreeMap::new(),
            body: Vec::new(),
            now_ms,
        }
    }

    /// Inserts a request header.
    #[must_use]
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers
            .insert(name.into().to_ascii_lowercase(), value.into());
        self
    }

    /// Sets raw byte body.
    #[must_use]
    pub fn with_body(mut self, body: Vec<u8>) -> Self {
        self.body = body;
        self
    }

    /// Serializes `payload` as a JSON request body.
    pub fn with_json<T: Serialize>(mut self, payload: &T) -> Result<Self, serde_json::Error> {
        self.body = serde_json::to_vec(payload)?;
        self.headers
            .insert("content-type".to_owned(), "application/json".to_owned());
        Ok(self)
    }

    /// Looks up a header case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        let target = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| k.to_ascii_lowercase() == target)
            .map(|(_, v)| v.as_str())
    }
}

/// Platform-agnostic HTTP response emitted by [`CloudflareWorkerRouter::dispatch`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerHttpResponse {
    /// HTTP status code (`200`, `202`, `204`, `400`, `404`, `405`, `413`, `429`, `503`, `507`).
    pub status: u16,
    /// Response headers.
    pub headers: BTreeMap<String, String>,
    /// Response body bytes.
    pub body: Vec<u8>,
}

impl WorkerHttpResponse {
    /// Builds an empty response with `status`.
    pub fn empty(status: u16) -> Self {
        Self {
            status,
            headers: BTreeMap::new(),
            body: Vec::new(),
        }
    }

    /// Builds a JSON response with `content-type: application/json` and `cache-control: no-store`.
    pub fn json<T: Serialize>(status: u16, value: &T) -> Self {
        let mut headers = BTreeMap::new();
        headers.insert("content-type".to_owned(), "application/json".to_owned());
        headers.insert("cache-control".to_owned(), "no-store".to_owned());
        let body = serde_json::to_vec(value).unwrap_or_default();
        Self {
            status,
            headers,
            body,
        }
    }

    /// Builds a JSON error response `{"error": "<code>"}`.
    pub fn error(status: u16, code: &str) -> Self {
        Self::json(status, &serde_json::json!({ "error": code }))
    }

    /// Decodes the JSON body into `T`.
    pub fn json_body<T: for<'de> Deserialize<'de>>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_slice(&self.body)
    }
}

/// `wasm32`-clean HTTP router unifying relay webhook inbox, Cloudflare API pause gate, and
/// task-cache / native-clients edge handlers backed by [`legion_edge`].
#[derive(Debug, Clone)]
pub struct CloudflareWorkerRouter {
    actors: BTreeMap<String, HookInboxDurableActor>,
    pause_gate: CloudflareApiPauseGate,
    admission: CacheAdmissionPolicy,
    ledger: D1BatchLedger,
    oidc: OidcCooldownTracker,
    multipart_parts: BTreeMap<String, BTreeMap<u32, String>>,
    stored_keys: BTreeMap<String, u64>,
    gc_batch_size: u32,
}

impl Default for CloudflareWorkerRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl CloudflareWorkerRouter {
    /// Creates a router with default `legion_edge` limits and empty state.
    pub fn new() -> Self {
        Self {
            actors: BTreeMap::new(),
            pause_gate: CloudflareApiPauseGate::new(),
            admission: CacheAdmissionPolicy::new(),
            ledger: D1BatchLedger::default(),
            oidc: OidcCooldownTracker::new(),
            multipart_parts: BTreeMap::new(),
            stored_keys: BTreeMap::new(),
            gc_batch_size: MAX_GC_BATCH_SIZE,
        }
    }

    /// Creates a router with custom D1 batch ledger quotas.
    #[must_use]
    pub fn with_ledger(mut self, ledger: D1BatchLedger) -> Self {
        self.ledger = ledger;
        self
    }

    /// Sets the D1 expired-generation GC batch limit (`1..=256`).
    #[must_use]
    pub fn with_gc_batch_size(mut self, batch_size: u32) -> Self {
        self.gc_batch_size = batch_size.clamp(1, MAX_GC_BATCH_SIZE);
        self
    }

    /// Borrows the [`HookInboxState`] for `endpoint_key`, if initialized.
    pub fn hook_inbox(&self, endpoint_key: &str) -> Option<&HookInboxState> {
        self.actors
            .get(endpoint_key)
            .map(HookInboxDurableActor::state)
    }

    /// Borrows the full [`HookInboxDurableActor`] for `endpoint_key`, if initialized.
    pub fn hook_actor(&self, endpoint_key: &str) -> Option<&HookInboxDurableActor> {
        self.actors.get(endpoint_key)
    }

    /// Borrows the [`CloudflareApiPauseGate`].
    pub fn pause_gate(&self) -> &CloudflareApiPauseGate {
        &self.pause_gate
    }

    /// Borrows the [`CacheAdmissionPolicy`].
    pub fn admission(&self) -> &CacheAdmissionPolicy {
        &self.admission
    }

    /// Borrows the [`D1BatchLedger`].
    pub fn ledger(&self) -> &D1BatchLedger {
        &self.ledger
    }

    /// Borrows the [`OidcCooldownTracker`].
    pub fn oidc_cooldown(&self) -> &OidcCooldownTracker {
        &self.oidc
    }

    /// Dispatches one HTTP request to the matching relay or task-cache handler.
    pub fn dispatch(&mut self, req: &WorkerHttpRequest) -> WorkerHttpResponse {
        self.dispatch_with_bindings(req, None).response
    }

    /// Dispatches one HTTP request with optional preloaded Cloudflare binding reads and returns
    /// both the HTTP response and planned [`EdgeHostEffect`]s.
    pub fn dispatch_with_bindings(
        &mut self,
        req: &WorkerHttpRequest,
        preloaded: Option<&BindingReadResults>,
    ) -> WorkerDispatchOutcome {
        if req.path == "/health" || req.path == "/v1/health" {
            if req.method != "GET" {
                return WorkerDispatchOutcome {
                    response: WorkerHttpResponse::error(405, "method_not_allowed"),
                    effects: Vec::new(),
                };
            }
            let pending_hooks: usize = self
                .actors
                .values()
                .map(HookInboxDurableActor::pending_count)
                .sum();
            return WorkerDispatchOutcome {
                response: WorkerHttpResponse::json(
                    200,
                    &serde_json::json!({
                        "ok": true,
                        "pendingHooks": pending_hooks,
                        "pausedUntilMs": self.pause_gate.paused_until_ms(),
                        "chargedBytes": self.ledger.charged_bytes(),
                    }),
                ),
                effects: vec![EdgeHostEffect::emit_metric(
                    "worker.health",
                    BTreeMap::new(),
                    1,
                )],
            };
        }

        if let Some(rest) = req.path.strip_prefix("/v1/hooks/") {
            let segs: Vec<&str> = rest.splitn(3, '/').collect();
            if segs.len() != 3 {
                return WorkerDispatchOutcome {
                    response: WorkerHttpResponse::error(404, "hook_not_found"),
                    effects: Vec::new(),
                };
            }
            return handle_public_hook(&mut self.actors, segs[0], segs[1], segs[2], req, preloaded);
        }

        if let Some(rest) = req.path.strip_prefix("/v1/internal/hooks/") {
            let Some((endpoint_key, action)) = rest.split_once('/') else {
                return WorkerDispatchOutcome {
                    response: WorkerHttpResponse::error(404, "unknown_route"),
                    effects: Vec::new(),
                };
            };
            return handle_internal_hook(&mut self.actors, endpoint_key, action, req, preloaded);
        }

        if let Some(action) = req.path.strip_prefix("/v1/internal/cloudflare-pause/") {
            return handle_pause_gate(&mut self.pause_gate, action, req, preloaded);
        }

        if let Some(tail) = req
            .path
            .strip_prefix("/projects/legioncode/native-clients/")
        {
            return handle_native_clients(
                &mut self.admission,
                &mut self.multipart_parts,
                tail,
                req,
                preloaded,
            );
        }

        if let Some(op) = req.path.strip_prefix("/projects/legioncode/") {
            return handle_task_cache(
                &mut self.admission,
                &mut self.ledger,
                &mut self.oidc,
                &mut self.stored_keys,
                op,
                req,
                preloaded,
            );
        }

        WorkerDispatchOutcome {
            response: WorkerHttpResponse::error(404, "unknown_route"),
            effects: Vec::new(),
        }
    }

    /// Evaluates a Cloudflare Worker scheduled cron trigger (`"* * * * *"` or `"0 * * * *"`).
    pub fn dispatch_scheduled(&mut self, cron: &str, now_ms: u64) -> ScheduledCronReport {
        run_scheduled_cron(cron, now_ms, self.gc_batch_size)
    }
}
