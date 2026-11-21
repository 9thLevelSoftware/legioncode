//! Durable Object HTTP `fetch_json` request dispatch and hold/outcome helpers for
//! [`crate::bridge_hook_actor::HookInboxDurableObjectBridge`].

use serde::Deserialize;
use t3_edge::cf_bindings::HookInboxDurableActor;
use t3_edge::relay_hooks::{DeliveryOutcome, HeldHook, HoldRefusalReason};

use crate::bridge_hook_actor_dto::{
    ActorFetchRequest, ActorSummaryOut, AlarmBatchOut, HoldHookRequest, RecordOutcomeOut,
    RecordOutcomeRequest,
};
use crate::bridge_worker_bindings::decode_body_bytes;

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct WakeRequest {
    #[serde(alias = "newBaseUrl", alias = "base_url", alias = "new_base_url")]
    base_url: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct AlarmFinishRequest {
    delivered: usize,
    #[serde(alias = "busy_hooks_count")]
    busy_hooks_count: usize,
}

pub(crate) fn apply_hold(
    actor: &mut HookInboxDurableActor,
    req: HoldHookRequest,
) -> Result<(String, u64), String> {
    let now_ms = req.now_ms.unwrap_or(0);
    let hook_key = req.hook_key.unwrap_or_else(|| "hook-1".to_owned());
    let delivery_id = req.id.unwrap_or_else(|| format!("{hook_key}-{now_ms}"));
    let method = req
        .method
        .unwrap_or_else(|| "POST".to_owned())
        .to_ascii_uppercase();
    let body = decode_body_bytes(req.body)?;
    let base_url = req
        .base_url
        .unwrap_or_else(|| "https://tunnel.invalid".to_owned());
    let mut hook = HeldHook::new(delivery_id.clone(), &hook_key, now_ms, method, body);
    if let Some(raw_hook_id) = req.raw_hook_id {
        hook.raw_hook_id = raw_hook_id;
    }
    if let Some(raw_token) = req.raw_token {
        hook.raw_token = raw_token;
    }
    if let Some(query) = req.query {
        hook.query = query;
    }
    hook.headers.extend(req.headers);

    let next_attempt = actor.hold(hook, base_url, now_ms).map_err(|reason| {
        let code = match reason {
            HoldRefusalReason::AlreadyHeld => "already_held",
            HoldRefusalReason::MaxRequests
            | HoldRefusalReason::MaxBytes
            | HoldRefusalReason::MaxPerHook => "inbox_capacity_exceeded",
        };
        format!("{code}: {reason}")
    })?;
    Ok((delivery_id, next_attempt))
}

pub(crate) fn dispatch_actor_fetch(
    actor: &mut HookInboxDurableActor,
    method: &str,
    path_only: &str,
    req: &ActorFetchRequest,
    body_bytes: &[u8],
) -> Result<(u16, serde_json::Value), String> {
    match (method, path_only) {
        ("GET", "/summary") => Ok((200_u16, serialize_value(&ActorSummaryOut::from(&*actor))?)),
        ("GET", "/state") => Ok((200_u16, serialize_value(&*actor)?)),
        ("POST", "/hold") => handle_fetch_hold(actor, req, body_bytes),
        ("POST", "/wake") => {
            let wake_req: WakeRequest = serde_json::from_slice(body_bytes)
                .map_err(|err| format!("invalid /wake payload: {err}"))?;
            let pending = actor.wake(wake_req.base_url, req.now_ms);
            Ok((
                200_u16,
                serde_json::json!({
                    "pending": pending,
                    "targetGeneration": actor.target_generation(),
                    "alarmAtMs": actor.alarm_at_ms(),
                }),
            ))
        }
        ("POST", "/deliver-batch") => {
            let batch = actor.begin_alarm_batch(req.now_ms);
            Ok((
                200_u16,
                serialize_value(&AlarmBatchOut::from_option(batch, actor))?,
            ))
        }
        ("POST", "/outcome") => handle_fetch_outcome(actor, req.now_ms, body_bytes),
        ("POST", "/finish-batch") => {
            let fin: AlarmFinishRequest = serde_json::from_slice(body_bytes)
                .map_err(|err| format!("invalid /finish-batch payload: {err}"))?;
            let next_alarm_at_ms =
                actor.finish_alarm_batch(fin.delivered, fin.busy_hooks_count, req.now_ms);
            Ok((
                200_u16,
                serde_json::json!({ "nextAlarmAtMs": next_alarm_at_ms }),
            ))
        }
        ("POST", "/run-failure") => {
            let retry_at_ms = actor.record_run_failure(req.now_ms);
            Ok((200_u16, serde_json::json!({ "retryAtMs": retry_at_ms })))
        }
        ("POST", "/clear") => {
            actor.clear();
            Ok((200_u16, serde_json::json!({ "cleared": true })))
        }
        _ => Ok((
            404_u16,
            serde_json::json!({ "error": "unknown_actor_route" }),
        )),
    }
}

fn handle_fetch_hold(
    actor: &mut HookInboxDurableActor,
    req: &ActorFetchRequest,
    body_bytes: &[u8],
) -> Result<(u16, serde_json::Value), String> {
    let mut hold_req: HoldHookRequest = if body_bytes.is_empty() {
        HoldHookRequest::default()
    } else {
        serde_json::from_slice(body_bytes).map_err(|err| format!("invalid /hold payload: {err}"))?
    };
    if hold_req.now_ms.is_none() {
        hold_req.now_ms = Some(req.now_ms);
    }
    if hold_req.base_url.is_none() {
        hold_req.base_url = req.headers.get("x-t3-target-url").cloned();
    }
    if hold_req.id.is_none() {
        hold_req.id = req.headers.get("x-t3-delivery-id").cloned();
    }
    match apply_hold(actor, hold_req) {
        Ok((delivery_id, next_attempt_at_ms)) => Ok((
            202_u16,
            serde_json::json!({
                "held": true,
                "deliveryId": delivery_id,
                "nextAttemptAtMs": next_attempt_at_ms,
                "targetGeneration": actor.target_generation(),
                "alarmAtMs": actor.alarm_at_ms(),
            }),
        )),
        Err(err) => {
            let code = if err.contains("already held") {
                409_u16
            } else {
                507_u16
            };
            Ok((code, serde_json::json!({ "error": err })))
        }
    }
}

fn handle_fetch_outcome(
    actor: &mut HookInboxDurableActor,
    fallback_now_ms: u64,
    body_bytes: &[u8],
) -> Result<(u16, serde_json::Value), String> {
    let mut outcome_req: RecordOutcomeRequest = serde_json::from_slice(body_bytes)
        .map_err(|err| format!("invalid /outcome payload: {err}"))?;
    if outcome_req.now_ms == 0 {
        outcome_req.now_ms = fallback_now_ms;
    }
    let outcome = outcome_req.outcome.unwrap_or_else(|| {
        outcome_req.status.map_or(
            DeliveryOutcome::Unreachable,
            DeliveryOutcome::from_http_status,
        )
    });
    let res = actor.record_delivery_outcome(
        outcome_req.snapshot_generation,
        &outcome_req.hook_id,
        outcome,
        outcome_req.now_ms,
    );
    Ok((
        200_u16,
        serialize_value(&RecordOutcomeOut::from_result(res, actor))?,
    ))
}

fn serialize_value<T: serde::Serialize>(value: &T) -> Result<serde_json::Value, String> {
    serde_json::to_value(value).map_err(|err| format!("failed to serialize actor body: {err}"))
}
