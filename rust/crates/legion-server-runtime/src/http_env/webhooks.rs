//! Webhook ingestion (`/api/hooks/{hookId}/{token}`).

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{OriginalUri, Path, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use rusqlite::{OptionalExtension, params};
use serde_json::json;
use uuid::Uuid;

use crate::http_api::RuntimeHttpState;
use crate::http_env::{constant_time_eq_bytes, env_auth_invalid, env_bad_request, env_error_json};
use crate::outbox_state::utc_now_iso;

const MAX_WEBHOOK_BODY_BYTES: usize = 1_048_576;

/// `GET|POST|PUT|PATCH /api/hooks/{hookId}/{token}` — accepts webhook deliveries (`202 { deliveryId }`).
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
