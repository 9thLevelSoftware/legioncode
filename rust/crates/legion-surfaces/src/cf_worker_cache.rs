//! Task-cache metadata, blob, telemetry, and OIDC edge route handlers for
//! [`crate::cf_worker::CloudflareWorkerRouter`].

use std::collections::BTreeMap;

use legion_edge::cf_bindings::{D1BindingPlan, R2BindingOp, R2BucketBinding};
use legion_edge::task_cache::{
    AdmissionKind, CacheAdmissionPolicy, D1BatchError, D1BatchLedger, D1LedgerOp,
    OidcCooldownTracker, SummaryRecord,
};
use serde::{Deserialize, Serialize};

use crate::cf_worker::cf_worker_bindings::{check_rate_limit, metric_tags};
use crate::cf_worker::{
    BindingReadResults, EdgeHostEffect, WorkerDispatchOutcome, WorkerHttpRequest,
    WorkerHttpResponse,
};

const CACHE_RATE_LIMIT_PER_MINUTE: u32 = 600;
const RATE_LIMIT_PERIOD_SECONDS: u32 = 60;

/// JSON payload for `POST /projects/legioncode/store`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheStorePayload {
    /// Cache key identifier.
    pub key: String,
    /// `true` when inserting a new primary entry row.
    pub key_is_new: bool,
    /// `true` when inserting a new association row.
    pub association_is_new: bool,
    /// Actual stored artifact size in bytes.
    pub actual_bytes: u64,
    /// Previously reserved capacity in bytes.
    pub reserved_bytes: u64,
    /// Bytes retired from an overwritten generation, if any.
    pub retired_bytes: Option<u64>,
}

/// JSON payload for `POST /projects/legioncode/fetch`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheFetchPayload {
    /// Cache key identifier.
    pub key: String,
    /// Rows scanned in D1.
    pub rows_read: u64,
    /// Whether matched via fallback association.
    pub fallback: bool,
}

pub(crate) fn handle_task_cache(
    admission: &mut CacheAdmissionPolicy,
    ledger: &mut D1BatchLedger,
    oidc: &mut OidcCooldownTracker,
    stored_keys: &mut BTreeMap<String, u64>,
    op: &str,
    req: &WorkerHttpRequest,
    preloaded: Option<&BindingReadResults>,
) -> WorkerDispatchOutcome {
    let mut effects = Vec::new();
    let response = if req.method == "POST" {
        match op {
            "oidc/refresh" => handle_oidc_refresh(oidc, req.now_ms, &mut effects),
            "store" => {
                handle_cache_store(admission, ledger, stored_keys, req, preloaded, &mut effects)
            }
            "fetch" => {
                handle_cache_fetch(admission, ledger, stored_keys, req, preloaded, &mut effects)
            }
            "telemetry" => handle_cache_telemetry(ledger, req, preloaded, &mut effects),
            _ => WorkerHttpResponse::error(404, "unknown_route"),
        }
    } else {
        WorkerHttpResponse::error(405, "method_not_allowed")
    };
    WorkerDispatchOutcome { response, effects }
}

fn handle_oidc_refresh(
    oidc: &mut OidcCooldownTracker,
    now_ms: u64,
    effects: &mut Vec<EdgeHostEffect>,
) -> WorkerHttpResponse {
    match oidc.try_start_refresh(now_ms) {
        Ok(()) => {
            effects.push(EdgeHostEffect::emit_metric(
                "task_cache.oidc_refresh",
                metric_tags([("allowed", "true")]),
                1,
            ));
            WorkerHttpResponse::json(200, &serde_json::json!({ "refreshed": true }))
        }
        Err(err) => {
            effects.push(EdgeHostEffect::emit_metric(
                "task_cache.oidc_refresh",
                metric_tags([("allowed", "false")]),
                1,
            ));
            let mut resp = WorkerHttpResponse::json(
                err.http_status(),
                &serde_json::json!({
                    "error": err.error_code(),
                    "retryAfterMs": err.retry_after_ms,
                }),
            );
            let secs = err.retry_after_ms.div_ceil(1_000).max(1);
            resp.headers
                .insert("retry-after".to_owned(), secs.to_string());
            resp
        }
    }
}

fn handle_cache_store(
    admission: &mut CacheAdmissionPolicy,
    ledger: &mut D1BatchLedger,
    stored_keys: &mut BTreeMap<String, u64>,
    req: &WorkerHttpRequest,
    preloaded: Option<&BindingReadResults>,
    effects: &mut Vec<EdgeHostEffect>,
) -> WorkerHttpResponse {
    let Ok(payload) = serde_json::from_slice::<CacheStorePayload>(&req.body) else {
        return WorkerHttpResponse::error(400, "invalid_payload");
    };
    if !check_rate_limit(
        effects,
        "STORE_LIMITER",
        "legioncode:store",
        CACHE_RATE_LIMIT_PER_MINUTE,
        RATE_LIMIT_PERIOD_SECONDS,
        preloaded,
    ) {
        return WorkerHttpResponse::error(429, "rate_limit");
    }
    let mut permit = match admission.try_acquire(AdmissionKind::Store) {
        Ok(p) => p,
        Err(rej) => return WorkerHttpResponse::error(rej.http_status(), rej.error_code()),
    };
    let ops = [
        D1LedgerOp::Reserve {
            bytes: payload.reserved_bytes,
        },
        D1LedgerOp::Publish {
            key_is_new: payload.key_is_new,
            association_is_new: payload.association_is_new,
            actual_bytes: payload.actual_bytes,
            reserved_bytes: payload.reserved_bytes,
            retired_bytes: payload.retired_bytes,
        },
    ];
    let res = ledger.execute_batch(&ops);
    admission.release(&mut permit);
    match res {
        Ok(receipt) => {
            let key_hex = hex::encode(payload.key.as_bytes());
            effects.push(EdgeHostEffect::r2_op(R2BindingOp::put(
                R2BucketBinding::Artifacts,
                format!("legioncode/{}/blob", payload.key),
                req.body.clone(),
            )));
            effects.push(EdgeHostEffect::d1_query(
                D1BindingPlan::upsert_artifact_entry("legioncode", &key_hex, &payload.key),
            ));
            effects.push(EdgeHostEffect::emit_metric(
                "task_cache.store",
                metric_tags([("key", payload.key.as_str())]),
                payload.actual_bytes,
            ));
            stored_keys.insert(payload.key, payload.actual_bytes);
            WorkerHttpResponse::json(
                200,
                &serde_json::json!({
                    "chargedBytes": ledger.charged_bytes(),
                    "entryCount": ledger.entry_count(),
                    "entriesApplied": receipt.entries_applied,
                }),
            )
        }
        Err(err @ D1BatchError::ByteLimitExceeded { .. }) => {
            WorkerHttpResponse::json(507, &serde_json::json!({ "error": err.to_string() }))
        }
        Err(err) => WorkerHttpResponse::json(400, &serde_json::json!({ "error": err.to_string() })),
    }
}

fn handle_cache_fetch(
    admission: &mut CacheAdmissionPolicy,
    ledger: &mut D1BatchLedger,
    stored_keys: &BTreeMap<String, u64>,
    req: &WorkerHttpRequest,
    preloaded: Option<&BindingReadResults>,
    effects: &mut Vec<EdgeHostEffect>,
) -> WorkerHttpResponse {
    let Ok(payload) = serde_json::from_slice::<CacheFetchPayload>(&req.body) else {
        return WorkerHttpResponse::error(400, "invalid_payload");
    };
    if !check_rate_limit(
        effects,
        "READ_LIMITER",
        "legioncode:fetch",
        CACHE_RATE_LIMIT_PER_MINUTE,
        RATE_LIMIT_PERIOD_SECONDS,
        preloaded,
    ) {
        return WorkerHttpResponse::error(429, "rate_limit");
    }
    let key_hex = hex::encode(payload.key.as_bytes());
    effects.push(EdgeHostEffect::d1_query(
        D1BindingPlan::select_artifact_entry("legioncode", &key_hex, &key_hex, req.now_ms / 1_000),
    ));
    let mut permit = match admission.try_acquire(AdmissionKind::Read) {
        Ok(p) => p,
        Err(rej) => return WorkerHttpResponse::error(rej.http_status(), rej.error_code()),
    };
    let (hit, fallback) = match preloaded.and_then(|p| p.d1_index_hit.as_ref()) {
        Some(Ok(Some(entry))) => (Some(entry.bytes), entry.fallback),
        Some(Ok(None)) => (None, payload.fallback),
        Some(Err(_)) => {
            admission.release(&mut permit);
            return WorkerHttpResponse::error(503, "unavailable");
        }
        None => (stored_keys.get(&payload.key).copied(), payload.fallback),
    };
    let op = match hit {
        Some(_) => D1LedgerOp::RecordHit {
            fallback,
            rows_read: payload.rows_read,
        },
        None => D1LedgerOp::RecordMiss {
            rows_read: payload.rows_read,
        },
    };
    let res = ledger.execute_batch(&[op]);
    admission.release(&mut permit);
    match res {
        Ok(_) => {
            if let Some(bytes) = hit {
                effects.push(EdgeHostEffect::r2_op(R2BindingOp::get(
                    R2BucketBinding::Artifacts,
                    format!("legioncode/{}/value", payload.key),
                )));
                effects.push(EdgeHostEffect::emit_metric(
                    "task_cache.fetch",
                    metric_tags([("hit", "true")]),
                    bytes,
                ));
            } else {
                effects.push(EdgeHostEffect::emit_metric(
                    "task_cache.fetch",
                    metric_tags([("hit", "false")]),
                    1,
                ));
            }
            WorkerHttpResponse::json(
                200,
                &serde_json::json!({
                    "hit": hit.is_some(),
                    "bytes": hit,
                    "hits": ledger.hits(),
                    "misses": ledger.misses(),
                }),
            )
        }
        Err(err) => WorkerHttpResponse::json(400, &serde_json::json!({ "error": err.to_string() })),
    }
}

fn handle_cache_telemetry(
    ledger: &mut D1BatchLedger,
    req: &WorkerHttpRequest,
    preloaded: Option<&BindingReadResults>,
    effects: &mut Vec<EdgeHostEffect>,
) -> WorkerHttpResponse {
    let Ok(summary) = serde_json::from_slice::<SummaryRecord>(&req.body) else {
        return WorkerHttpResponse::error(400, "invalid_summary");
    };
    if !check_rate_limit(
        effects,
        "READ_LIMITER",
        "legioncode:telemetry",
        CACHE_RATE_LIMIT_PER_MINUTE,
        RATE_LIMIT_PERIOD_SECONDS,
        preloaded,
    ) {
        return WorkerHttpResponse::error(429, "rate_limit");
    }
    let id = summary.id.clone();
    let recorded_at_ms = summary.recorded_at_ms;
    let developer = summary.developer.clone();
    let tasks_len = summary.tasks.len() as u64;
    let summary_for_plan = summary.clone();
    match ledger.execute_batch(&[D1LedgerOp::RecordSavings { summary }]) {
        Ok(_) => {
            effects.push(EdgeHostEffect::d1_query(
                D1BindingPlan::insert_savings_receipt(&id, recorded_at_ms),
            ));
            effects.push(EdgeHostEffect::d1_query(
                D1BindingPlan::upsert_savings_rollup(&summary_for_plan, &developer),
            ));
            effects.push(EdgeHostEffect::emit_metric(
                "task_cache.telemetry",
                metric_tags([("developer", developer.as_str())]),
                tasks_len,
            ));
            WorkerHttpResponse::empty(204)
        }
        Err(err) => WorkerHttpResponse::json(400, &serde_json::json!({ "error": err.to_string() })),
    }
}
