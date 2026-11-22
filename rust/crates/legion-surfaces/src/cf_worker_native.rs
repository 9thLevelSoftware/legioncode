//! Native mobile dev client R2 multipart route handlers for
//! [`crate::cf_worker::CloudflareWorkerRouter`].

use std::collections::BTreeMap;

use legion_edge::cf_bindings::{R2BindingOp, R2BucketBinding};
use legion_edge::task_cache::{
    AdmissionKind, CacheAdmissionPolicy, CompletedPart, MAX_COMPLETE_BODY_BYTES, parse_part_number,
    validate_complete_parts,
};
use serde::Deserialize;

use crate::cf_worker::cf_worker_bindings::{check_rate_limit, metric_tags};
use crate::cf_worker::{
    BindingReadResults, EdgeHostEffect, WorkerDispatchOutcome, WorkerHttpRequest,
    WorkerHttpResponse,
};

const NATIVE_RATE_LIMIT_PER_MINUTE: u32 = 600;
const RATE_LIMIT_PERIOD_SECONDS: u32 = 60;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompletePartsPayload {
    parts: Vec<CompletedPart>,
}

struct NativeUploadTarget<'a> {
    r2_key: &'a str,
    upload_id: &'a str,
    op: &'a str,
}

pub(crate) fn handle_native_clients(
    admission: &mut CacheAdmissionPolicy,
    parts: &mut BTreeMap<String, BTreeMap<u32, String>>,
    tail: &str,
    req: &WorkerHttpRequest,
    preloaded: Option<&BindingReadResults>,
) -> WorkerDispatchOutcome {
    let mut effects = Vec::new();
    let segs: Vec<&str> = tail.split('/').collect();
    let response = match segs.as_slice() {
        [platform, variant, fingerprint] => {
            let r2_key = format!("{platform}/{variant}/{fingerprint}");
            handle_native_download(&r2_key, req, preloaded, &mut effects)
        }
        [platform, variant, fingerprint, "uploads"] => {
            let r2_key = format!("{platform}/{variant}/{fingerprint}");
            handle_native_create(&r2_key, req, preloaded, &mut effects)
        }
        [platform, variant, fingerprint, "uploads", upload_id] => {
            let r2_key = format!("{platform}/{variant}/{fingerprint}");
            handle_native_abort(parts, &r2_key, upload_id, req, preloaded, &mut effects)
        }
        [platform, variant, fingerprint, "uploads", upload_id, op] => {
            let r2_key = format!("{platform}/{variant}/{fingerprint}");
            let target = NativeUploadTarget {
                r2_key: &r2_key,
                upload_id,
                op,
            };
            handle_native_part_or_complete(admission, parts, &target, req, preloaded, &mut effects)
        }
        _ => WorkerHttpResponse::error(404, "unknown_route"),
    };
    WorkerDispatchOutcome { response, effects }
}

fn handle_native_download(
    r2_key: &str,
    req: &WorkerHttpRequest,
    preloaded: Option<&BindingReadResults>,
    effects: &mut Vec<EdgeHostEffect>,
) -> WorkerHttpResponse {
    if req.method != "GET" {
        return WorkerHttpResponse::error(405, "method_not_allowed");
    }
    if !check_rate_limit(
        effects,
        "READ_LIMITER",
        "legioncode:native-read",
        NATIVE_RATE_LIMIT_PER_MINUTE,
        RATE_LIMIT_PERIOD_SECONDS,
        preloaded,
    ) {
        return WorkerHttpResponse::error(429, "rate_limit");
    }
    effects.push(EdgeHostEffect::r2_op(R2BindingOp::get(
        R2BucketBinding::Clients,
        r2_key,
    )));
    let Some(bytes) = preloaded.and_then(|p| p.r2_object_bytes.as_ref()) else {
        effects.push(EdgeHostEffect::emit_metric(
            "native_clients.download",
            metric_tags([("hit", "false")]),
            1,
        ));
        return WorkerHttpResponse::error(404, "missing");
    };
    let len = bytes.len() as u64;
    let mut headers = BTreeMap::new();
    headers.insert(
        "content-type".to_owned(),
        "application/octet-stream".to_owned(),
    );
    headers.insert("content-length".to_owned(), len.to_string());
    headers.insert("cache-control".to_owned(), "no-store".to_owned());
    effects.push(EdgeHostEffect::emit_metric(
        "native_clients.download",
        metric_tags([("hit", "true")]),
        len,
    ));
    WorkerHttpResponse {
        status: 200,
        headers,
        body: bytes.clone(),
    }
}

fn handle_native_create(
    r2_key: &str,
    req: &WorkerHttpRequest,
    preloaded: Option<&BindingReadResults>,
    effects: &mut Vec<EdgeHostEffect>,
) -> WorkerHttpResponse {
    if req.method != "POST" {
        return WorkerHttpResponse::error(405, "method_not_allowed");
    }
    if !check_rate_limit(
        effects,
        "STORE_LIMITER",
        "legioncode:native-store",
        NATIVE_RATE_LIMIT_PER_MINUTE,
        RATE_LIMIT_PERIOD_SECONDS,
        preloaded,
    ) {
        return WorkerHttpResponse::error(429, "rate_limit");
    }
    let digest = req.header("x-legioncode-digest").unwrap_or("");
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        return WorkerHttpResponse::error(400, "invalid_digest");
    }
    let mut meta = BTreeMap::new();
    meta.insert("digest".to_owned(), digest.to_owned());
    effects.push(EdgeHostEffect::r2_op(R2BindingOp::create_multipart_upload(
        R2BucketBinding::Clients,
        r2_key,
        meta,
    )));
    let upload_id = format!("up-{}", req.now_ms);
    effects.push(EdgeHostEffect::emit_metric(
        "native_clients.create",
        metric_tags([("key", r2_key)]),
        1,
    ));
    WorkerHttpResponse::json(200, &serde_json::json!({ "uploadId": upload_id }))
}

fn handle_native_abort(
    parts: &mut BTreeMap<String, BTreeMap<u32, String>>,
    r2_key: &str,
    upload_id: &str,
    req: &WorkerHttpRequest,
    preloaded: Option<&BindingReadResults>,
    effects: &mut Vec<EdgeHostEffect>,
) -> WorkerHttpResponse {
    if req.method != "DELETE" {
        return WorkerHttpResponse::error(405, "method_not_allowed");
    }
    if !check_rate_limit(
        effects,
        "STORE_LIMITER",
        "legioncode:native-store",
        NATIVE_RATE_LIMIT_PER_MINUTE,
        RATE_LIMIT_PERIOD_SECONDS,
        preloaded,
    ) {
        return WorkerHttpResponse::error(429, "rate_limit");
    }
    parts.remove(&format!("{r2_key}/{upload_id}"));
    effects.push(EdgeHostEffect::r2_op(R2BindingOp::abort_multipart_upload(
        R2BucketBinding::Clients,
        r2_key,
        upload_id,
    )));
    effects.push(EdgeHostEffect::emit_metric(
        "native_clients.abort",
        metric_tags([("key", r2_key)]),
        1,
    ));
    WorkerHttpResponse::empty(204)
}

fn handle_native_part_or_complete(
    admission: &mut CacheAdmissionPolicy,
    parts: &mut BTreeMap<String, BTreeMap<u32, String>>,
    target: &NativeUploadTarget<'_>,
    req: &WorkerHttpRequest,
    preloaded: Option<&BindingReadResults>,
    effects: &mut Vec<EdgeHostEffect>,
) -> WorkerHttpResponse {
    let key = format!("{}/{}", target.r2_key, target.upload_id);
    if target.op == "complete" {
        if req.method != "POST" {
            return WorkerHttpResponse::error(405, "method_not_allowed");
        }
        if !check_rate_limit(
            effects,
            "STORE_LIMITER",
            "legioncode:native-store",
            NATIVE_RATE_LIMIT_PER_MINUTE,
            RATE_LIMIT_PERIOD_SECONDS,
            preloaded,
        ) {
            return WorkerHttpResponse::error(429, "rate_limit");
        }
        if req.body.len() > MAX_COMPLETE_BODY_BYTES {
            return WorkerHttpResponse::error(413, "parts_too_large");
        }
        let Ok(payload) = serde_json::from_slice::<CompletePartsPayload>(&req.body) else {
            return WorkerHttpResponse::error(400, "invalid_parts");
        };
        if let Err(err) = validate_complete_parts(&payload.parts) {
            return WorkerHttpResponse::error(err.http_status(), err.error_code());
        }
        if let Ok(r2_op) = R2BindingOp::complete_multipart_upload(
            R2BucketBinding::Clients,
            target.r2_key,
            target.upload_id,
            payload.parts,
        ) {
            effects.push(EdgeHostEffect::r2_op(r2_op));
        }
        parts.remove(&key);
        effects.push(EdgeHostEffect::emit_metric(
            "native_clients.complete",
            metric_tags([("key", target.r2_key)]),
            1,
        ));
        return WorkerHttpResponse::empty(204);
    }

    if req.method != "PUT" {
        return WorkerHttpResponse::error(405, "method_not_allowed");
    }
    if !check_rate_limit(
        effects,
        "STORE_LIMITER",
        "legioncode:native-store",
        NATIVE_RATE_LIMIT_PER_MINUTE,
        RATE_LIMIT_PERIOD_SECONDS,
        preloaded,
    ) {
        return WorkerHttpResponse::error(429, "rate_limit");
    }
    let part_num = match parse_part_number(target.op) {
        Ok(num) => num,
        Err(err) => return WorkerHttpResponse::error(err.http_status(), err.error_code()),
    };
    let r2_op = match R2BindingOp::upload_part(
        R2BucketBinding::Clients,
        target.r2_key,
        target.upload_id,
        part_num,
        req.body.clone(),
    ) {
        Ok(built) => built,
        Err(err) => return WorkerHttpResponse::error(err.http_status(), err.error_code()),
    };
    let mut permit = match admission.try_acquire(AdmissionKind::Store) {
        Ok(p) => p,
        Err(rej) => return WorkerHttpResponse::error(rej.http_status(), rej.error_code()),
    };
    let etag = format!("etag-{part_num}-{}", req.body.len());
    parts.entry(key).or_default().insert(part_num, etag.clone());
    admission.release(&mut permit);
    effects.push(EdgeHostEffect::r2_op(r2_op));
    effects.push(EdgeHostEffect::emit_metric(
        "native_clients.upload_part",
        metric_tags([("part", target.op)]),
        req.body.len() as u64,
    ));
    WorkerHttpResponse::json(
        200,
        &serde_json::json!({ "partNumber": part_num, "etag": etag }),
    )
}
