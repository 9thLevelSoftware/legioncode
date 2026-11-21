//! Request/response serialization and preloaded binding normalization for
//! [`crate::bridge_worker::EdgeWorkerBridge`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use t3_edge::cf_bindings::HookInboxDurableActor;
use t3_edge::task_cache::SavingsTotals;
use t3_surfaces::cf_worker::{
    BindingReadResults, EdgeHostEffect, PreloadedCacheHit, WorkerHttpRequest, WorkerHttpResponse,
};

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum HyperdrivePauseInput {
    Result(Result<Option<u64>, String>),
    Direct(u64),
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum D1IndexHitInput {
    Result(Result<Option<PreloadedCacheHit>, String>),
    Direct(PreloadedCacheHit),
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct PreloadedBindingsInput {
    #[serde(alias = "rate_limit_allowed")]
    pub rate_limit_allowed: Option<bool>,
    #[serde(alias = "hyperdrive_paused_until_ms")]
    pub hyperdrive_paused_until_ms: Option<HyperdrivePauseInput>,
    #[serde(alias = "hyperdrive_read_error")]
    pub hyperdrive_read_error: Option<String>,
    #[serde(alias = "d1_index_hit")]
    pub d1_index_hit: Option<D1IndexHitInput>,
    #[serde(alias = "d1_index_error")]
    pub d1_index_error: Option<String>,
    #[serde(alias = "d1_savings_totals")]
    pub d1_savings_totals: Option<SavingsTotals>,
    #[serde(alias = "r2_object_bytes")]
    pub r2_object_bytes: Option<Vec<u8>>,
    #[serde(alias = "r2_object_text")]
    pub r2_object_text: Option<String>,
    #[serde(alias = "relay_environment_link_active")]
    pub relay_environment_link_active: Option<bool>,
    #[serde(alias = "hook_actor")]
    pub hook_actor: Option<HookInboxDurableActor>,
}

impl PreloadedBindingsInput {
    pub fn into_binding_reads(self) -> BindingReadResults {
        let hyperdrive_paused_until_ms = if let Some(err) = self.hyperdrive_read_error {
            Some(Err(err))
        } else {
            self.hyperdrive_paused_until_ms.map(|val| match val {
                HyperdrivePauseInput::Result(res) => res,
                HyperdrivePauseInput::Direct(ms) => Ok(Some(ms)),
            })
        };
        let d1_index_hit = if let Some(err) = self.d1_index_error {
            Some(Err(err))
        } else {
            self.d1_index_hit.map(|val| match val {
                D1IndexHitInput::Result(res) => res,
                D1IndexHitInput::Direct(hit) => Ok(Some(hit)),
            })
        };
        let r2_object_bytes = self
            .r2_object_bytes
            .or_else(|| self.r2_object_text.map(String::into_bytes));
        BindingReadResults {
            rate_limit_allowed: self.rate_limit_allowed,
            hyperdrive_paused_until_ms,
            d1_index_hit,
            d1_savings_totals: self.d1_savings_totals,
            r2_object_bytes,
            relay_environment_link_active: self.relay_environment_link_active,
            hook_actor: self.hook_actor,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct WorkerRequestDto {
    pub method: Option<String>,
    pub path: String,
    pub query: Option<String>,
    pub headers: BTreeMap<String, String>,
    pub body: Option<serde_json::Value>,
    #[serde(alias = "payload")]
    pub body_json: Option<serde_json::Value>,
    #[serde(alias = "now_ms")]
    pub now_ms: u64,
    #[serde(alias = "endpoint_key")]
    pub endpoint_key: Option<String>,
    #[serde(alias = "hook_id")]
    pub hook_id: Option<String>,
    pub token: Option<String>,
    #[serde(alias = "bindings", alias = "bindingReads", alias = "binding_reads")]
    pub preloaded: Option<PreloadedBindingsInput>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkerResponseDto {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: Option<serde_json::Value>,
    pub body_text: String,
    pub body_bytes: Vec<u8>,
}

impl From<WorkerHttpResponse> for WorkerResponseDto {
    fn from(resp: WorkerHttpResponse) -> Self {
        let parsed_body = if resp.body.is_empty() {
            None
        } else {
            serde_json::from_slice::<serde_json::Value>(&resp.body).ok()
        };
        let body_text = String::from_utf8_lossy(&resp.body).into_owned();
        Self {
            status: resp.status,
            headers: resp.headers,
            body: parsed_body,
            body_text,
            body_bytes: resp.body,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkerDispatchOutcomeDto {
    pub response: WorkerResponseDto,
    pub effects: Vec<EdgeHostEffect>,
}

pub(crate) fn build_worker_request(
    dto: WorkerRequestDto,
) -> Result<(WorkerHttpRequest, Option<BindingReadResults>), String> {
    if dto.path.trim().is_empty() {
        return Err("edge request `path` cannot be empty".to_owned());
    }
    let (raw_path, inline_query) = match dto.path.split_once('?') {
        Some((path_only, query)) => (path_only, Some(query.to_owned())),
        None => (dto.path.as_str(), None),
    };
    let normalized_path = normalize_worker_path(
        raw_path,
        dto.endpoint_key.as_deref(),
        dto.hook_id.as_deref(),
        dto.token.as_deref(),
    );
    let method = dto.method.map_or_else(
        || {
            if normalized_path == "/health" || normalized_path == "/v1/health" {
                "GET".to_owned()
            } else {
                "POST".to_owned()
            }
        },
        |m| m.to_ascii_uppercase(),
    );
    let body = decode_body_bytes(dto.body.or(dto.body_json))?;
    let query = dto.query.or(inline_query);

    let mut worker_req = WorkerHttpRequest::new(method, normalized_path, dto.now_ms);
    worker_req.query = query;
    for (name, value) in dto.headers {
        worker_req = worker_req.with_header(name, value);
    }
    worker_req = worker_req.with_body(body);
    let preloaded = dto
        .preloaded
        .map(PreloadedBindingsInput::into_binding_reads);
    Ok((worker_req, preloaded))
}

pub(crate) fn normalize_worker_path(
    raw_path: &str,
    endpoint_key: Option<&str>,
    hook_id: Option<&str>,
    token: Option<&str>,
) -> String {
    let ep = endpoint_key.unwrap_or("default");
    match raw_path {
        "/hooks/wake" => format!("/v1/internal/hooks/{ep}/wake"),
        "/hooks/outcome" => format!("/v1/internal/hooks/{ep}/outcome"),
        "/hooks/deliver-batch" => format!("/v1/internal/hooks/{ep}/deliver-batch"),
        "/hooks/hold" => {
            let hid = hook_id.unwrap_or("hook");
            let tok = token.unwrap_or("token");
            format!("/v1/hooks/{ep}/{hid}/{tok}")
        }
        "/pause/evaluate" => "/v1/internal/cloudflare-pause/evaluate".to_owned(),
        "/pause/rate-limit" => "/v1/internal/cloudflare-pause/rate-limit".to_owned(),
        "/cache/store" => "/projects/t3code/store".to_owned(),
        "/cache/fetch" => "/projects/t3code/fetch".to_owned(),
        "/cache/telemetry" => "/projects/t3code/telemetry".to_owned(),
        "/cache/oidc/refresh" => "/projects/t3code/oidc/refresh".to_owned(),
        other => {
            if let Some(rest) = other.strip_prefix("/hooks/") {
                let segs: Vec<&str> = rest.split('/').collect();
                if segs.len() == 2 && matches!(segs[1], "wake" | "outcome" | "deliver-batch") {
                    return format!("/v1/internal/hooks/{rest}");
                }
                if segs.len() == 3 {
                    return format!("/v1/hooks/{rest}");
                }
            }
            other.to_owned()
        }
    }
}

pub(crate) fn decode_body_bytes(value: Option<serde_json::Value>) -> Result<Vec<u8>, String> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(serde_json::Value::String(text)) => Ok(text.into_bytes()),
        Some(serde_json::Value::Array(items)) => {
            let maybe_bytes: Option<Vec<u8>> = items
                .iter()
                .map(|item| item.as_u64().and_then(|n| u8::try_from(n).ok()))
                .collect();
            if let Some(bytes) = maybe_bytes {
                Ok(bytes)
            } else {
                serde_json::to_vec(&serde_json::Value::Array(items))
                    .map_err(|err| format!("failed to encode array body: {err}"))
            }
        }
        Some(other) => {
            serde_json::to_vec(&other).map_err(|err| format!("failed to encode JSON body: {err}"))
        }
    }
}
