//! Read-only peer thread SSE/HTTP routes (`/api/links/thread/*`, `/api/peer/threads*`) (`#39406`).

use std::convert::Infallible;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::header::{CACHE_CONTROL, HeaderName, HeaderValue};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use legion_contracts::peer_links::LINK_EXPIRES_AT_HEADER;
use legion_contracts::permissions::EnvironmentScope;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::ServerRuntimeError;
use crate::http_api::RuntimeHttpState;
use crate::http_auth::{AuthenticatedPrincipal, authenticate_headers};
use crate::http_auth_guard::authorize_request;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PeerThreadQuery {
    pub thread: Option<String>,
    pub thread_id: Option<String>,
    pub cursor: Option<String>,
    pub item: Option<String>,
    pub item_id: Option<String>,
}

fn refusal(status: StatusCode, error: &str) -> Response {
    let mut resp = (status, Json(json!({ "error": error }))).into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

fn attach_expiry_and_no_store(headers: &mut HeaderMap, expires_at: Option<&str>) {
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Some(exp) = expires_at
        && let Ok(name) = HeaderName::from_bytes(LINK_EXPIRES_AT_HEADER.as_bytes())
        && let Ok(val) = HeaderValue::from_str(exp)
    {
        headers.insert(name, val);
    }
}

fn admit_link_thread_request(
    state: &RuntimeHttpState,
    headers: &HeaderMap,
    require_no_origin: bool,
) -> Result<Option<String>, Box<Response>> {
    if require_no_origin && headers.get(axum::http::header::ORIGIN).is_some() {
        return Err(Box::new(refusal(
            StatusCode::FORBIDDEN,
            "origin_not_allowed",
        )));
    }
    if !state.linked_environments_enabled {
        return Err(Box::new(refusal(
            StatusCode::FORBIDDEN,
            "linked_environments_off",
        )));
    }
    let principal: AuthenticatedPrincipal = authenticate_headers(state, headers)
        .map_err(|_| Box::new(refusal(StatusCode::UNAUTHORIZED, "unauthorized")))?;
    if !principal
        .scopes
        .iter()
        .any(|s| s == "*" || s == EnvironmentScope::OrchestrationRead.as_str())
    {
        return Err(Box::new(refusal(
            StatusCode::FORBIDDEN,
            "insufficient_scope",
        )));
    }
    Ok(None)
}

fn read_local_thread_snapshot(
    state: &RuntimeHttpState,
    thread_id: &str,
) -> Result<Value, StatusCode> {
    let conn = state
        .db()
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let row = conn.query_row(
        "SELECT project_id, title, COALESCE(provider_instance_id, default_provider),
                COALESCE(json_extract(payload_json, '$.modelSelection.model'), ''),
                updated_at
         FROM orchestration_v2_projection_threads
         WHERE thread_id = ?1 AND deleted_at IS NULL",
        [thread_id],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        },
    );
    let Ok((project_id, title, provider, model, updated_at)) = row else {
        return Err(StatusCode::NOT_FOUND);
    };

    let mut msg_stmt = conn
        .prepare(
            "SELECT message_id, run_id, role, COALESCE(json_extract(payload_json, '$.text'), ''), created_at
             FROM orchestration_v2_projection_messages
             WHERE thread_id = ?1
             ORDER BY created_at ASC, message_id ASC",
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let messages: Vec<Value> = msg_stmt
        .query_map([thread_id], |r| {
            Ok(json!({
                "itemId": r.get::<_, String>(0)?,
                "runId": r.get::<_, Option<String>>(1)?,
                "role": r.get::<_, String>(2)?,
                "text": r.get::<_, String>(3)?,
                "createdAt": r.get::<_, String>(4)?,
            }))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(json!({
        "kind": "snapshot",
        "thread": {
            "threadId": thread_id,
            "projectId": project_id,
            "title": title,
            "providerInstanceId": provider,
            "model": model,
            "updatedAt": updated_at,
        },
        "items": messages,
    }))
}

fn read_local_thread_history(
    state: &RuntimeHttpState,
    thread_id: &str,
    cursor: &str,
) -> Result<Value, StatusCode> {
    let snapshot = read_local_thread_snapshot(state, thread_id)?;
    let offset = cursor.parse::<usize>().unwrap_or(0);
    let items = snapshot["items"].as_array().cloned().unwrap_or_default();
    let sliced: Vec<Value> = items.into_iter().skip(offset).take(50).collect();
    Ok(json!({
        "threadId": thread_id,
        "cursor": cursor,
        "items": sliced,
    }))
}

fn read_local_thread_item(
    state: &RuntimeHttpState,
    thread_id: &str,
    item_id: &str,
) -> Result<Value, StatusCode> {
    let conn = state
        .db()
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let row = conn.query_row(
        "SELECT message_id, run_id, role, payload_json, created_at
         FROM orchestration_v2_projection_messages
         WHERE thread_id = ?1 AND message_id = ?2",
        [thread_id, item_id],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        },
    );
    let Ok((mid, run_id, role, payload_json, created_at)) = row else {
        return Err(StatusCode::NOT_FOUND);
    };
    let payload: Value = serde_json::from_str(&payload_json).unwrap_or_else(|_| json!({}));
    Ok(json!({
        "threadId": thread_id,
        "item": {
            "itemId": mid,
            "runId": run_id,
            "role": role,
            "payload": payload,
            "createdAt": created_at,
        }
    }))
}

/// `GET /api/peer/threads` — read-only list of local threads exposed to peers/UI (`OrchestrationRead`).
pub(crate) async fn handle_list_peer_threads(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
) -> Result<Response, ServerRuntimeError> {
    let _principal = authorize_request(
        &state,
        &headers,
        &[EnvironmentScope::OrchestrationRead],
        false,
    )?;
    let conn = state
        .db()
        .lock()
        .map_err(|_| ServerRuntimeError::Persistence("db mutex poisoned".to_owned()))?;
    let mut stmt = conn.prepare(
        "SELECT thread_id, project_id, title, COALESCE(provider_instance_id, default_provider), updated_at
         FROM orchestration_v2_projection_threads
         WHERE deleted_at IS NULL AND archived_at IS NULL
         ORDER BY updated_at DESC, thread_id ASC
         LIMIT 200",
    )?;
    let threads: Vec<Value> = stmt
        .query_map([], |r| {
            Ok(json!({
                "threadId": r.get::<_, String>(0)?,
                "projectId": r.get::<_, String>(1)?,
                "title": r.get::<_, String>(2)?,
                "providerInstanceId": r.get::<_, String>(3)?,
                "updatedAt": r.get::<_, String>(4)?,
            }))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok((StatusCode::OK, Json(json!({ "threads": threads }))).into_response())
}

pub(crate) async fn handle_link_thread_stream(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    Query(query): Query<PeerThreadQuery>,
) -> Response {
    serve_thread_stream(&state, &headers, query, None, true)
}

pub(crate) async fn handle_peer_thread_stream(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    Path(thread_id): Path<String>,
    Query(query): Query<PeerThreadQuery>,
) -> Response {
    serve_thread_stream(&state, &headers, query, Some(thread_id), false)
}

fn serve_thread_stream(
    state: &RuntimeHttpState,
    headers: &HeaderMap,
    query: PeerThreadQuery,
    path_thread: Option<String>,
    require_no_origin: bool,
) -> Response {
    let expires_at = match admit_link_thread_request(state, headers, require_no_origin) {
        Ok(exp) => exp,
        Err(resp) => return *resp,
    };
    let Some(thread_id) = path_thread.or(query.thread).or(query.thread_id) else {
        return refusal(StatusCode::BAD_REQUEST, "invalid_request");
    };
    let snapshot = match read_local_thread_snapshot(state, &thread_id) {
        Ok(snap) => snap,
        Err(StatusCode::NOT_FOUND) => return refusal(StatusCode::NOT_FOUND, "thread_not_found"),
        Err(_) => return refusal(StatusCode::INTERNAL_SERVER_ERROR, "thread_read_failed"),
    };
    let event_data = serde_json::to_string(&snapshot).unwrap_or_else(|_| "{}".to_owned());
    let stream = futures_util::stream::once(async move {
        Ok::<Event, Infallible>(Event::default().data(event_data))
    });
    let mut resp = Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response();
    attach_expiry_and_no_store(resp.headers_mut(), expires_at.as_deref());
    resp
}

pub(crate) async fn handle_link_thread_history(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    Query(query): Query<PeerThreadQuery>,
) -> Response {
    let expires_at = match admit_link_thread_request(&state, &headers, true) {
        Ok(exp) => exp,
        Err(resp) => return *resp,
    };
    let (Some(thread_id), Some(cursor)) = (query.thread.or(query.thread_id), query.cursor) else {
        return refusal(StatusCode::BAD_REQUEST, "invalid_request");
    };
    match read_local_thread_history(&state, &thread_id, &cursor) {
        Ok(body) => {
            let mut resp = (StatusCode::OK, Json(body)).into_response();
            attach_expiry_and_no_store(resp.headers_mut(), expires_at.as_deref());
            resp
        }
        Err(StatusCode::NOT_FOUND) => refusal(StatusCode::NOT_FOUND, "thread_not_found"),
        Err(_) => refusal(StatusCode::INTERNAL_SERVER_ERROR, "thread_read_failed"),
    }
}

pub(crate) async fn handle_link_thread_item(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    Query(query): Query<PeerThreadQuery>,
) -> Response {
    let expires_at = match admit_link_thread_request(&state, &headers, true) {
        Ok(exp) => exp,
        Err(resp) => return *resp,
    };
    let (Some(thread_id), Some(item_id)) = (
        query.thread.or(query.thread_id),
        query.item.or(query.item_id),
    ) else {
        return refusal(StatusCode::BAD_REQUEST, "invalid_request");
    };
    match read_local_thread_item(&state, &thread_id, &item_id) {
        Ok(body) => {
            let mut resp = (StatusCode::OK, Json(body)).into_response();
            attach_expiry_and_no_store(resp.headers_mut(), expires_at.as_deref());
            resp
        }
        Err(StatusCode::NOT_FOUND) => refusal(StatusCode::NOT_FOUND, "thread_not_found"),
        Err(_) => refusal(StatusCode::INTERNAL_SERVER_ERROR, "thread_read_failed"),
    }
}
