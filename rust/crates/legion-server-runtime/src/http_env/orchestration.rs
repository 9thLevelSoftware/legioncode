//! HTTP orchestration snapshot, thread detail, bounded paging, history, and transcript handlers.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use legion_contracts::permissions::EnvironmentScope;
use legion_orchestrator::projections::{
    ProjectionError, WireShellOptions, WireShellStreamEvent, build_wire_shell_patch_from_events,
    query_wire_shell_http_snapshot_with_options, query_wire_thread_bounded,
    query_wire_thread_detail, query_wire_thread_history_page, query_wire_thread_transcript,
};
use serde::Deserialize;
use serde_json::json;

use crate::http_api::RuntimeHttpState;
use crate::http_env::{
    EnvErr, EnvResult, authorize_env, env_bad_request, env_internal, env_invalid_request,
    env_not_found,
};

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShellSnapshotQuery {
    omit_idle_subagents: Option<String>,
    omit_older_settled: Option<String>,
    resume_from: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadWindowQuery {
    limit: Option<String>,
    before: Option<String>,
}

fn validate_protocol_header(headers: &HeaderMap) -> EnvResult<()> {
    if let Some(raw) = headers.get("x-legioncode-orchestration-protocol") {
        let value = raw.to_str().unwrap_or("").trim();
        if value != "2" {
            return Err(EnvErr(Box::new(env_invalid_request(
                "invalid_command",
                format!("Unsupported orchestration protocol version `{value}`; expected `2`"),
            ))));
        }
    }
    Ok(())
}

fn parse_bool_param(raw: Option<&str>, default_val: bool) -> bool {
    match raw.map(str::trim) {
        Some(v) if v.eq_ignore_ascii_case("true") || v == "1" => true,
        Some(v) if v.eq_ignore_ascii_case("false") || v == "0" => false,
        _ => default_val,
    }
}

fn parse_opt_limit(raw: Option<&str>) -> EnvResult<Option<usize>> {
    let Some(val) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let parsed = val.parse::<usize>().map_err(|_| {
        EnvErr(Box::new(env_bad_request(format!(
            "Invalid limit parameter `{val}`"
        ))))
    })?;
    if parsed == 0 {
        return Err(EnvErr(Box::new(env_bad_request(
            "limit must be greater than 0",
        ))));
    }
    Ok(Some(parsed))
}

fn map_projection_error(err: ProjectionError) -> EnvErr {
    let resp = match err {
        ProjectionError::ThreadNotFound(id) => env_not_found(format!("Thread `{id}` not found")),
        ProjectionError::InvalidCursor => {
            env_invalid_request("invalid_history_cursor", "Invalid thread history cursor")
        }
        other => env_internal(other.to_string()),
    };
    EnvErr(Box::new(resp))
}

/// `GET /api/orchestration/shell` — returns `{ resumable: true }` or `OrchestrationV2ShellHttpSnapshot`.
pub(crate) async fn handle_shell_snapshot(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    Query(query): Query<ShellSnapshotQuery>,
) -> EnvResult<Response> {
    authorize_env(&state, &headers, Some(EnvironmentScope::OrchestrationRead))?;
    validate_protocol_header(&headers)?;

    let resume_seq = match query
        .resume_from
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(raw) => match raw.parse::<i64>() {
            Ok(seq) if seq >= 0 => Some(seq),
            _ => {
                return Err(EnvErr(Box::new(env_bad_request(format!(
                    "Invalid resumeFrom sequence `{raw}`"
                )))));
            }
        },
        None => None,
    };

    let options = WireShellOptions {
        omit_idle_subagents: parse_bool_param(query.omit_idle_subagents.as_deref(), true),
        omit_older_settled: parse_bool_param(query.omit_older_settled.as_deref(), true),
    };

    let conn = state
        .db()
        .lock()
        .map_err(|_| EnvErr(Box::new(env_internal("database lock poisoned"))))?;

    if let Some(after_seq) = resume_seq {
        let events = build_wire_shell_patch_from_events(&conn, after_seq, "", None)
            .map_err(map_projection_error)?;
        if !matches!(events.first(), Some(WireShellStreamEvent::SnapshotRequired)) {
            return Ok(Json(json!({ "resumable": true })).into_response());
        }
    }

    let snapshot = query_wire_shell_http_snapshot_with_options(&conn, "", options)
        .map_err(map_projection_error)?;
    Ok(Json(snapshot).into_response())
}

/// `GET /api/orchestration/threads/{threadId}` — full `OrchestrationV2ThreadDetailSnapshot`.
pub(crate) async fn handle_thread_detail(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    Path(thread_id): Path<String>,
) -> EnvResult<Response> {
    authorize_env(&state, &headers, Some(EnvironmentScope::OrchestrationRead))?;
    validate_protocol_header(&headers)?;
    let conn = state
        .db()
        .lock()
        .map_err(|_| EnvErr(Box::new(env_internal("database lock poisoned"))))?;
    let detail = query_wire_thread_detail(&conn, &thread_id).map_err(map_projection_error)?;
    Ok(Json(detail).into_response())
}

/// `GET /api/orchestration/threads/{threadId}/transcript` — structured JSON transcript.
pub(crate) async fn handle_thread_transcript(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    Path(thread_id): Path<String>,
) -> EnvResult<Response> {
    authorize_env(&state, &headers, Some(EnvironmentScope::OrchestrationRead))?;
    validate_protocol_header(&headers)?;
    let conn = state
        .db()
        .lock()
        .map_err(|_| EnvErr(Box::new(env_internal("database lock poisoned"))))?;
    let transcript =
        query_wire_thread_transcript(&conn, &thread_id).map_err(map_projection_error)?;
    Ok(Json(transcript).into_response())
}

/// `GET /api/orchestration/threads/{threadId}/bounded` — bounded window snapshot with keyset cursor.
pub(crate) async fn handle_thread_bounded(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    Path(thread_id): Path<String>,
    Query(query): Query<ThreadWindowQuery>,
) -> EnvResult<Response> {
    authorize_env(&state, &headers, Some(EnvironmentScope::OrchestrationRead))?;
    validate_protocol_header(&headers)?;
    let limit = parse_opt_limit(query.limit.as_deref())?;
    let conn = state
        .db()
        .lock()
        .map_err(|_| EnvErr(Box::new(env_internal("database lock poisoned"))))?;
    let snapshot =
        query_wire_thread_bounded(&conn, &thread_id, limit).map_err(map_projection_error)?;
    Ok(Json(snapshot).into_response())
}

/// `GET /api/orchestration/threads/{threadId}/history` — strictly older timeline history page.
pub(crate) async fn handle_thread_history(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    Path(thread_id): Path<String>,
    Query(query): Query<ThreadWindowQuery>,
) -> EnvResult<Response> {
    authorize_env(&state, &headers, Some(EnvironmentScope::OrchestrationRead))?;
    validate_protocol_header(&headers)?;
    let limit = parse_opt_limit(query.limit.as_deref())?;
    let before = query
        .before
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let conn = state
        .db()
        .lock()
        .map_err(|_| EnvErr(Box::new(env_internal("database lock poisoned"))))?;
    let page = query_wire_thread_history_page(&conn, &thread_id, before, limit)
        .map_err(map_projection_error)?;
    Ok(Json(page).into_response())
}
