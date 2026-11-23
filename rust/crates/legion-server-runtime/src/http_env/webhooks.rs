//! Webhook ingestion (`/api/hooks/{hookId}/{token}`).

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{OriginalUri, Path, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use legion_workflows::triggers_hooks::{HOOK_PREFIX, HookDeliveryInput, WebhookRequest};
use legion_workflows::types::WorkflowError;
use rusqlite::{OptionalExtension, params};
use serde_json::json;
use uuid::Uuid;

use crate::http_api::RuntimeHttpState;
use crate::http_env::{constant_time_eq_bytes, env_auth_invalid, env_bad_request, env_error_json};
use crate::outbox_state::utc_now_iso;

const MAX_WEBHOOK_BODY_BYTES: usize = 1_048_576;

/// `GET|POST|PUT|PATCH /api/hooks/{hookId}/{token}` (and `/webhooks/{hookId}/{token}`) —
/// routes `wf-*` hooks to [`WorkflowEngine::deliver`] and scheduled-task hooks to SQLite (`#38882`).
pub(crate) async fn handle_webhook_delivery(
    State(state): State<Arc<RuntimeHttpState>>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    Path((hook_id, token)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !matches!(
        method,
        Method::GET | Method::POST | Method::PUT | Method::PATCH
    ) {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    if hook_id.trim().is_empty() || token.trim().is_empty() {
        return env_bad_request("Missing webhook hookId or token");
    }
    if body.len() > MAX_WEBHOOK_BODY_BYTES {
        return env_error_json(
            StatusCode::PAYLOAD_TOO_LARGE,
            json!({
                "_tag": "EnvironmentHttpBadRequestError",
                "message": "Webhook payload exceeds 1 MiB limit",
            }),
        );
    }

    if hook_id.starts_with(HOOK_PREFIX) {
        return dispatch_workflow_webhook(&state, &method, &uri, &hook_id, &token, &headers, &body)
            .await;
    }

    let delivery_id = Uuid::new_v4().to_string();
    let now_iso = utc_now_iso();
    if let Ok(conn) = state.db().lock() {
        let task_token: Option<Option<String>> = conn
            .query_row(
                "SELECT webhook_token FROM scheduled_tasks WHERE task_id = ?1 LIMIT 1",
                [hook_id.as_str()],
                |r| r.get(0),
            )
            .optional()
            .unwrap_or(None);

        if let Some(Some(expected_token)) = task_token {
            if !constant_time_eq_bytes(expected_token.as_bytes(), token.as_bytes()) {
                return env_auth_invalid("invalid_credential", "Invalid webhook token");
            }
            let headers_json = headers
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .map_or_else(
                    || "{}".to_owned(),
                    |ct| json!({ "content-type": ct }).to_string(),
                );
            let body_text = String::from_utf8_lossy(&body);
            let _ = conn.execute(
                "INSERT INTO scheduled_task_webhook_deliveries (
                   delivery_id, task_id, received_at, method, query, headers_json,
                   body, body_bytes, body_truncated, outcome, signature_verified
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, 'accepted', 1)",
                params![
                    delivery_id,
                    hook_id,
                    now_iso,
                    method.as_str(),
                    uri.query().unwrap_or(""),
                    headers_json,
                    body_text.as_ref(),
                    i64::try_from(body.len()).unwrap_or(i64::MAX),
                ],
            );
        }
    }

    let mut resp = (
        StatusCode::ACCEPTED,
        Json(json!({ "deliveryId": delivery_id })),
    )
        .into_response();
    resp.headers_mut().insert(
        "x-legioncode-hook-outcome",
        HeaderValue::from_static("accepted"),
    );
    resp
}

fn hook_json_response(
    status: StatusCode,
    payload: serde_json::Value,
    outcome: &'static str,
) -> Response {
    let mut resp = (status, Json(payload)).into_response();
    resp.headers_mut().insert(
        "x-legioncode-hook-outcome",
        HeaderValue::from_static(outcome),
    );
    resp
}

async fn dispatch_workflow_webhook(
    state: &RuntimeHttpState,
    method: &Method,
    uri: &axum::http::Uri,
    hook_id: &str,
    token: &str,
    headers: &HeaderMap,
    body: &Bytes,
) -> Response {
    let policy_denied = state
        .managed_policy
        .as_ref()
        .and_then(|p| p.limits.as_ref())
        .is_some_and(|l| l.webhook_tasks == Some(false));
    if policy_denied {
        return hook_json_response(
            StatusCode::FORBIDDEN,
            json!({ "error": "blocked_by_policy" }),
            "blocked_by_policy",
        );
    }
    let Some(engine) = state.workflow_engine() else {
        return hook_json_response(
            StatusCode::NOT_FOUND,
            json!({ "error": "hook_not_found" }),
            "not_found",
        );
    };
    let mut header_map = BTreeMap::new();
    for (k, v) in headers {
        if let Ok(val) = v.to_str() {
            header_map.insert(k.as_str().to_ascii_lowercase(), val.to_owned());
        }
    }
    let relay_delivery_id = headers
        .get("x-legioncode-relay-delivery-id")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!("relay:{s}"));
    let input = HookDeliveryInput {
        hook_id: hook_id.to_owned(),
        token: token.to_owned(),
        request: WebhookRequest {
            method: method.as_str().to_owned(),
            headers: header_map,
            query: uri.query().unwrap_or_default().to_owned(),
            body: String::from_utf8_lossy(body).into_owned(),
        },
        delivery_id: relay_delivery_id,
    };
    match engine.deliver(input).await {
        Ok(Some(run_id)) => hook_json_response(
            StatusCode::ACCEPTED,
            json!({ "runId": run_id, "deliveryId": run_id }),
            "accepted",
        ),
        Ok(None) => hook_json_response(
            StatusCode::NOT_FOUND,
            json!({ "error": "hook_not_found" }),
            "not_found",
        ),
        Err(WorkflowError::Input { problem }) => hook_json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "invalid_request", "problem": problem }),
            "invalid",
        ),
        Err(_) => hook_json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "internal_error" }),
            "error",
        ),
    }
}
