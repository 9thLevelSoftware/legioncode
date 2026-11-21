//! Relay webhook inbox Durable Object route handlers for
//! [`crate::cf_worker::CloudflareWorkerRouter`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use t3_edge::cf_bindings::{HookInboxDurableActor, HyperdriveRelayPlan};
use t3_edge::relay_hooks::{DeliveryOutcome, HeldHook, HoldRefusalReason};

use crate::cf_worker::cf_worker_bindings::{check_rate_limit, metric_tags};
use crate::cf_worker::{
    BindingReadResults, EdgeHostEffect, WorkerDispatchOutcome, WorkerHttpRequest,
    WorkerHttpResponse,
};

const HOOK_MAX_BODY_BYTES: usize = 1_048_576;
const HOOK_RATE_LIMIT_PER_MINUTE: u32 = 600;
const RATE_LIMIT_PERIOD_SECONDS: u32 = 60;

/// JSON payload for `POST /v1/internal/hooks/{endpoint_key}/wake`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WakePayload {
    /// New environment tunnel base URL.
    pub base_url: String,
}

/// JSON payload for `POST /v1/internal/hooks/{endpoint_key}/outcome`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookOutcomePayload {
    /// Snapshot `target_generation` captured before sending upstream (`BUG-29`).
    pub snapshot_generation: u64,
    /// Delivery identifier.
    pub hook_id: String,
    /// Upstream HTTP status code (mapped via `DeliveryOutcome::from_http_status`) or `None` on
    /// transport/timeout failure.
    pub status: Option<u16>,
}

pub(crate) fn handle_public_hook(
    actors: &mut BTreeMap<String, HookInboxDurableActor>,
    endpoint_key: &str,
    hook_id: &str,
    token: &str,
    req: &WorkerHttpRequest,
    preloaded: Option<&BindingReadResults>,
) -> WorkerDispatchOutcome {
    let mut effects = Vec::new();
    if !matches!(req.method.as_str(), "GET" | "POST" | "PUT" | "PATCH") {
        let mut response = WorkerHttpResponse::error(405, "method_not_allowed");
        response
            .headers
            .insert("allow".to_owned(), "GET, POST, PUT, PATCH".to_owned());
        return WorkerDispatchOutcome { response, effects };
    }
    if is_unsafe_segment(endpoint_key, false)
        || is_unsafe_segment(hook_id, false)
        || is_unsafe_segment(token, true)
    {
        return WorkerDispatchOutcome {
            response: WorkerHttpResponse::error(404, "hook_not_found"),
            effects,
        };
    }
    if req.body.len() > HOOK_MAX_BODY_BYTES {
        return WorkerDispatchOutcome {
            response: WorkerHttpResponse::error(413, "payload_too_large"),
            effects,
        };
    }
    if !check_rate_limit(
        &mut effects,
        "HOOK_RATE_LIMIT",
        endpoint_key,
        HOOK_RATE_LIMIT_PER_MINUTE,
        RATE_LIMIT_PERIOD_SECONDS,
        preloaded,
    ) {
        return WorkerDispatchOutcome {
            response: WorkerHttpResponse::error(429, "rate_limit"),
            effects,
        };
    }
    effects.push(EdgeHostEffect::hyperdrive_query(
        HyperdriveRelayPlan::find_active_managed_links(endpoint_key),
    ));
    if matches!(
        preloaded.and_then(|p| p.relay_environment_link_active),
        Some(false)
    ) {
        effects.push(EdgeHostEffect::emit_metric(
            "relay.hook.unlinked",
            metric_tags([("endpoint", endpoint_key)]),
            1,
        ));
        return WorkerDispatchOutcome {
            response: WorkerHttpResponse::error(404, "hook_not_found"),
            effects,
        };
    }

    let actor = actors.entry(endpoint_key.to_owned()).or_insert_with(|| {
        preloaded
            .and_then(|p| p.hook_actor.clone())
            .unwrap_or_else(|| HookInboxDurableActor::new(endpoint_key))
    });
    let response = hold_validated_hook(actor, endpoint_key, hook_id, token, req, &mut effects);
    WorkerDispatchOutcome { response, effects }
}

fn hold_validated_hook(
    actor: &mut HookInboxDurableActor,
    endpoint_key: &str,
    hook_id: &str,
    token: &str,
    req: &WorkerHttpRequest,
    effects: &mut Vec<EdgeHostEffect>,
) -> WorkerHttpResponse {
    let delivery_id = req
        .header("x-t3-delivery-id")
        .map_or_else(|| format!("{hook_id}-{}", req.now_ms), str::to_owned);
    let base_url = req
        .header("x-t3-target-url")
        .unwrap_or("https://tunnel.invalid");
    let mut hook = HeldHook::new(
        delivery_id.clone(),
        hook_id,
        req.now_ms,
        req.method.clone(),
        req.body.clone(),
    );
    hook_id.clone_into(&mut hook.raw_hook_id);
    token.clone_into(&mut hook.raw_token);
    hook.query = req.query.clone().unwrap_or_default();

    match actor.hold(hook, base_url, req.now_ms) {
        Ok(next_attempt_at_ms) => {
            effects.push(EdgeHostEffect::durable_object_call(
                "HookInboxObject",
                endpoint_key,
                "hold",
                Some(actor.clone()),
            ));
            effects.push(EdgeHostEffect::emit_metric(
                "relay.hook.held",
                metric_tags([("endpoint", endpoint_key)]),
                1,
            ));
            WorkerHttpResponse::json(
                202,
                &serde_json::json!({
                    "held": true,
                    "deliveryId": delivery_id,
                    "nextAttemptAtMs": next_attempt_at_ms,
                    "targetGeneration": actor.target_generation(),
                }),
            )
        }
        Err(HoldRefusalReason::AlreadyHeld) => WorkerHttpResponse::error(409, "already_held"),
        Err(
            HoldRefusalReason::MaxRequests
            | HoldRefusalReason::MaxBytes
            | HoldRefusalReason::MaxPerHook,
        ) => WorkerHttpResponse::error(507, "inbox_capacity_exceeded"),
    }
}

pub(crate) fn handle_internal_hook(
    actors: &mut BTreeMap<String, HookInboxDurableActor>,
    endpoint_key: &str,
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
    let actor = actors.entry(endpoint_key.to_owned()).or_insert_with(|| {
        preloaded
            .and_then(|p| p.hook_actor.clone())
            .unwrap_or_else(|| HookInboxDurableActor::new(endpoint_key))
    });
    let response = match action {
        "wake" => handle_wake_action(actor, endpoint_key, req, &mut effects),
        "deliver-batch" => {
            handle_deliver_batch_action(actor, endpoint_key, req.now_ms, &mut effects)
        }
        "outcome" => handle_outcome_action(actor, endpoint_key, req, &mut effects),
        _ => WorkerHttpResponse::error(404, "unknown_route"),
    };
    WorkerDispatchOutcome { response, effects }
}

fn handle_wake_action(
    actor: &mut HookInboxDurableActor,
    endpoint_key: &str,
    req: &WorkerHttpRequest,
    effects: &mut Vec<EdgeHostEffect>,
) -> WorkerHttpResponse {
    let Ok(payload) = serde_json::from_slice::<WakePayload>(&req.body) else {
        return WorkerHttpResponse::error(400, "invalid_payload");
    };
    let pending = actor.wake(payload.base_url, req.now_ms);
    effects.push(EdgeHostEffect::durable_object_call(
        "HookInboxObject",
        endpoint_key,
        "wake",
        Some(actor.clone()),
    ));
    effects.push(EdgeHostEffect::emit_metric(
        "relay.hook.wake",
        metric_tags([("pending", if pending { "true" } else { "false" })]),
        1,
    ));
    WorkerHttpResponse::json(
        200,
        &serde_json::json!({
            "pending": pending,
            "targetGeneration": actor.target_generation(),
        }),
    )
}

fn handle_deliver_batch_action(
    actor: &mut HookInboxDurableActor,
    endpoint_key: &str,
    now_ms: u64,
    effects: &mut Vec<EdgeHostEffect>,
) -> WorkerHttpResponse {
    let batch = actor.begin_alarm_batch(now_ms);
    effects.push(EdgeHostEffect::durable_object_call(
        "HookInboxObject",
        endpoint_key,
        "deliver-batch",
        Some(actor.clone()),
    ));
    let count = batch.as_ref().map_or(0, |b| b.hooks.len() as u64);
    effects.push(EdgeHostEffect::emit_metric(
        "relay.hook.deliver_batch",
        metric_tags([("endpoint", endpoint_key)]),
        count,
    ));
    match batch {
        Some(batch) => WorkerHttpResponse::json(
            200,
            &serde_json::json!({
                "targetGeneration": batch.target_generation,
                "baseUrl": batch.base_url,
                "hookIds": batch.hooks.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(),
            }),
        ),
        None => WorkerHttpResponse::json(200, &serde_json::json!({ "empty": true })),
    }
}

fn handle_outcome_action(
    actor: &mut HookInboxDurableActor,
    endpoint_key: &str,
    req: &WorkerHttpRequest,
    effects: &mut Vec<EdgeHostEffect>,
) -> WorkerHttpResponse {
    let Ok(payload) = serde_json::from_slice::<HookOutcomePayload>(&req.body) else {
        return WorkerHttpResponse::error(400, "invalid_payload");
    };
    let outcome = payload.status.map_or(
        DeliveryOutcome::Unreachable,
        DeliveryOutcome::from_http_status,
    );
    let recorded = actor.record_delivery_outcome(
        payload.snapshot_generation,
        &payload.hook_id,
        outcome,
        req.now_ms,
    );
    effects.push(EdgeHostEffect::durable_object_call(
        "HookInboxObject",
        endpoint_key,
        "outcome",
        Some(actor.clone()),
    ));
    let stale_tag = if recorded.is_ignored_stale() {
        "true"
    } else {
        "false"
    };
    effects.push(EdgeHostEffect::emit_metric(
        "relay.hook.outcome",
        metric_tags([("ignored_stale", stale_tag)]),
        1,
    ));
    WorkerHttpResponse::json(
        200,
        &serde_json::json!({
            "ignoredStale": recorded.is_ignored_stale(),
            "pendingCount": actor.pending_count(),
            "failures": actor.state().target().map_or(0, |t| t.failures),
        }),
    )
}

fn is_unsafe_segment(segment: &str, allow_slash: bool) -> bool {
    segment.is_empty()
        || segment == "."
        || segment == ".."
        || segment.contains('\\')
        || (!allow_slash && segment.contains('/'))
}
