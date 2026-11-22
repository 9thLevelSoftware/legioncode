//! Pure-Rust host-testable bridge for [`legion_surfaces::cf_worker::CloudflareWorkerRouter`].

use legion_surfaces::cf_worker::CloudflareWorkerRouter;

use crate::bridge_worker_bindings::{
    WorkerDispatchOutcomeDto, WorkerRequestDto, WorkerResponseDto, build_worker_request,
};

/// Host-testable Cloudflare Worker router wrapper backing `WasmEdgeWorker`.
#[derive(Debug, Clone)]
pub struct EdgeWorkerBridge {
    router: CloudflareWorkerRouter,
}

impl Default for EdgeWorkerBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl EdgeWorkerBridge {
    /// Creates an [`EdgeWorkerBridge`] with fresh [`CloudflareWorkerRouter`] state.
    pub fn new() -> Self {
        Self {
            router: CloudflareWorkerRouter::new(),
        }
    }

    /// Borrows the underlying [`CloudflareWorkerRouter`].
    pub const fn router(&self) -> &CloudflareWorkerRouter {
        &self.router
    }

    /// Dispatches a JSON-encoded HTTP request envelope through [`CloudflareWorkerRouter`].
    ///
    /// Accepts both full canonical paths (`/v1/internal/hooks/{ep}/wake`,
    /// `/v1/internal/hooks/{ep}/outcome`, `/v1/internal/cloudflare-pause/evaluate`,
    /// `/projects/legioncode/store`, `/projects/legioncode/fetch`, `/v1/hooks/{ep}/{id}/{tok}`, `/health`)
    /// and shorthand paths (`/hooks/wake`, `/hooks/outcome`, `/hooks/deliver-batch`,
    /// `/hooks/hold`, `/pause/evaluate`, `/pause/rate-limit`, `/cache/store`, `/cache/fetch`,
    /// `/cache/telemetry`, `/cache/oidc/refresh`).
    pub fn handle_request_json(&mut self, req_json: &str) -> Result<String, String> {
        let dto: WorkerRequestDto = serde_json::from_str(req_json)
            .map_err(|err| format!("invalid edge request JSON: {err}"))?;
        let (worker_req, _) = build_worker_request(dto)?;
        let out = WorkerResponseDto::from(self.router.dispatch(&worker_req));
        serde_json::to_string(&out).map_err(|err| format!("failed to serialize response: {err}"))
    }

    /// Dispatches one JSON HTTP request envelope with optional preloaded Cloudflare binding reads
    /// (`preloaded`) and returns `{ "response": ..., "effects": [...] }`.
    pub fn dispatch_with_bindings_json(&mut self, req_json: &str) -> Result<String, String> {
        let dto: WorkerRequestDto = serde_json::from_str(req_json)
            .map_err(|err| format!("invalid edge bindings request JSON: {err}"))?;
        let (worker_req, preloaded) = build_worker_request(dto)?;
        let outcome = self
            .router
            .dispatch_with_bindings(&worker_req, preloaded.as_ref());
        let out = WorkerDispatchOutcomeDto {
            response: WorkerResponseDto::from(outcome.response),
            effects: outcome.effects,
        };
        serde_json::to_string(&out)
            .map_err(|err| format!("failed to serialize dispatch outcome: {err}"))
    }

    /// Evaluates a scheduled Cloudflare Worker cron trigger (`"* * * * *"` or `"0 * * * *"`)
    /// at explicit epoch milliseconds `now_ms`.
    pub fn handle_scheduled_json(&mut self, cron: &str, now_ms: u64) -> Result<String, String> {
        let report = self.router.dispatch_scheduled(cron, now_ms);
        serde_json::to_string(&report)
            .map_err(|err| format!("failed to serialize scheduled cron report: {err}"))
    }
}
