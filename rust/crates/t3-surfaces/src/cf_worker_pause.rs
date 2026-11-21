//! Shared Cloudflare API `429` pause gate route handlers for
//! [`crate::cf_worker::CloudflareWorkerRouter`].

use serde::{Deserialize, Serialize};
use t3_edge::cf_bindings::{CLOUDFLARE_UPSTREAM_API_KEY, HyperdriveRelayPlan};
use t3_edge::relay_hooks::CloudflareApiPauseGate;

use crate::cf_worker::cf_worker_bindings::metric_tags;
use crate::cf_worker::{
    BindingReadResults, EdgeHostEffect, WorkerDispatchOutcome, WorkerHttpRequest,
    WorkerHttpResponse,
};

/// JSON payload for `POST /v1/internal/cloudflare-pause/evaluate` (`BUG-32`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PauseEvaluatePayload {
    /// Simulated shared store result: `Some(until_ms)` on success, `None` when `read_error` set.
    pub shared_paused_until_ms: Option<u64>,
    /// Simulated shared store error string (when present, `checked_at_ms` must NOT advance).
    pub shared_read_error: Option<String>,
}

/// JSON payload for `POST /v1/internal/cloudflare-pause/rate-limit`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitPayload {
    /// Optional upstream `Retry-After` duration in milliseconds.
    pub retry_after_ms: Option<u64>,
}

pub(crate) fn handle_pause_gate(
    gate: &mut CloudflareApiPauseGate,
    action: &str,
    req: &WorkerHttpRequest,
    preloaded: Option<&BindingReadResults>,
) -> WorkerDispatchOutcome {
    let mut effects = Vec::new();
    if req.method != "POST" {
        return WorkerDispatchOutcome {
            response: WorkerHttpResponse::error(405, "method_not_allowed"),
            effects,
        };
    }
    let response = match action {
        "evaluate" => evaluate_pause_action(gate, req, preloaded, &mut effects),
        "rate-limit" => {
            let Ok(payload) = serde_json::from_slice::<RateLimitPayload>(&req.body) else {
                return WorkerDispatchOutcome {
                    response: WorkerHttpResponse::error(400, "invalid_payload"),
                    effects,
                };
            };
            let until = gate.record_rate_limit(req.now_ms, payload.retry_after_ms);
            effects.push(EdgeHostEffect::hyperdrive_query(
                HyperdriveRelayPlan::record_upstream_api_pause(CLOUDFLARE_UPSTREAM_API_KEY, until),
            ));
            effects.push(EdgeHostEffect::emit_metric(
                "relay.pause_gate.rate_limit",
                metric_tags([("api", CLOUDFLARE_UPSTREAM_API_KEY)]),
                until,
            ));
            WorkerHttpResponse::json(200, &serde_json::json!({ "pausedUntilMs": until }))
        }
        _ => WorkerHttpResponse::error(404, "unknown_route"),
    };
    WorkerDispatchOutcome { response, effects }
}

fn evaluate_pause_action(
    gate: &mut CloudflareApiPauseGate,
    req: &WorkerHttpRequest,
    preloaded: Option<&BindingReadResults>,
    effects: &mut Vec<EdgeHostEffect>,
) -> WorkerHttpResponse {
    let preloaded_pause = preloaded.and_then(|p| p.hyperdrive_paused_until_ms.as_ref());
    let payload = if req.body.is_empty() && preloaded_pause.is_some() {
        PauseEvaluatePayload {
            shared_paused_until_ms: None,
            shared_read_error: None,
        }
    } else {
        let Ok(parsed) = serde_json::from_slice::<PauseEvaluatePayload>(&req.body) else {
            return WorkerHttpResponse::error(400, "invalid_payload");
        };
        parsed
    };
    if gate.should_refresh(req.now_ms) {
        effects.push(EdgeHostEffect::hyperdrive_query(
            HyperdriveRelayPlan::read_upstream_api_pause(CLOUDFLARE_UPSTREAM_API_KEY),
        ));
    }
    let result = gate.evaluate(req.now_ms, || match preloaded_pause {
        Some(Ok(until_opt)) => Ok(until_opt.unwrap_or(0)),
        Some(Err(err)) => Err(err.clone()),
        None => match payload.shared_read_error {
            Some(err) => Err(err),
            None => Ok(payload.shared_paused_until_ms.unwrap_or(0)),
        },
    });
    match result {
        Ok(pass) => {
            effects.push(EdgeHostEffect::emit_metric(
                "relay.pause_gate.evaluate",
                metric_tags([
                    ("paused", "false"),
                    (
                        "refreshed",
                        if pass.refreshed_from_shared {
                            "true"
                        } else {
                            "false"
                        },
                    ),
                ]),
                1,
            ));
            WorkerHttpResponse::json(
                200,
                &serde_json::json!({
                    "refreshedFromShared": pass.refreshed_from_shared,
                    "sharedReadError": pass.shared_read_error,
                    "checkedAtMs": gate.checked_at_ms(),
                    "pausedUntilMs": gate.paused_until_ms(),
                }),
            )
        }
        Err(paused) => {
            effects.push(EdgeHostEffect::emit_metric(
                "relay.pause_gate.evaluate",
                metric_tags([("paused", "true")]),
                paused.retry_after_ms,
            ));
            let mut resp = WorkerHttpResponse::json(
                503,
                &serde_json::json!({
                    "error": "cloudflare_api_paused",
                    "retryAfterMs": paused.retry_after_ms,
                    "checkedAtMs": gate.checked_at_ms(),
                }),
            );
            let secs = paused.retry_after_ms.div_ceil(1_000).max(1);
            resp.headers
                .insert("retry-after".to_owned(), secs.to_string());
            resp
        }
    }
}
