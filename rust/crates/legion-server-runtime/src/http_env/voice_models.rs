//! Voice model management, acceleration, and unload-timeout handlers (`/api/voice/models/*`).

use std::path::PathBuf;
use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use legion_contracts::permissions::EnvironmentScope;
use legion_speech::inspect_model_file;
use serde::Deserialize;
use serde_json::json;

use crate::http_api::RuntimeHttpState;
use crate::http_env::{
    EnvErr, EnvResult, authorize_env, env_internal, env_invalid_request, parse_json_body,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelIdBody {
    model_id: String,
}

#[derive(Debug, Deserialize)]
struct ModelLinkBody {
    path: String,
}

#[derive(Debug, Deserialize)]
struct AccelerationBody {
    acceleration: String,
}

#[derive(Debug, Deserialize)]
struct UnloadTimeoutBody {
    timeout: String,
}

/// `POST /api/voice/models/prepare`
pub(crate) async fn handle_voice_models_prepare(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
) -> EnvResult<Response> {
    authorize_env(
        &state,
        &headers,
        Some(EnvironmentScope::OrchestrationOperate),
    )?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `GET /api/voice/models`
pub(crate) async fn handle_voice_models_list(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
) -> EnvResult<Response> {
    authorize_env(&state, &headers, Some(EnvironmentScope::OrchestrationRead))?;
    let guard = state
        .voice_state
        .lock()
        .map_err(|_| EnvErr(Box::new(env_internal("voice state lock poisoned"))))?;
    Ok(Json(json!({ "models": guard.catalog() })).into_response())
}

/// `POST /api/voice/models/download`
pub(crate) async fn handle_voice_models_download(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> EnvResult<Response> {
    authorize_env(
        &state,
        &headers,
        Some(EnvironmentScope::OrchestrationOperate),
    )?;
    let req: ModelIdBody = parse_json_body(&body)?;
    let mut guard = state
        .voice_state
        .lock()
        .map_err(|_| EnvErr(Box::new(env_internal("voice state lock poisoned"))))?;
    guard.installed_models.insert(req.model_id.clone());
    guard.active_model_id = req.model_id;
    Ok(Json(json!({ "models": guard.catalog(), "cancelled": false })).into_response())
}

/// `POST /api/voice/models/select`
pub(crate) async fn handle_voice_models_select(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> EnvResult<Response> {
    authorize_env(
        &state,
        &headers,
        Some(EnvironmentScope::OrchestrationOperate),
    )?;
    let req: ModelIdBody = parse_json_body(&body)?;
    let mut guard = state
        .voice_state
        .lock()
        .map_err(|_| EnvErr(Box::new(env_internal("voice state lock poisoned"))))?;
    guard.active_model_id = req.model_id;
    Ok(Json(json!({ "models": guard.catalog() })).into_response())
}

/// `POST /api/voice/models/cancel`
pub(crate) async fn handle_voice_models_cancel(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> EnvResult<Response> {
    authorize_env(
        &state,
        &headers,
        Some(EnvironmentScope::OrchestrationOperate),
    )?;
    let _req: ModelIdBody = parse_json_body(&body)?;
    let guard = state
        .voice_state
        .lock()
        .map_err(|_| EnvErr(Box::new(env_internal("voice state lock poisoned"))))?;
    Ok(Json(json!({ "models": guard.catalog() })).into_response())
}

/// `POST /api/voice/models/remove`
pub(crate) async fn handle_voice_models_remove(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> EnvResult<Response> {
    authorize_env(
        &state,
        &headers,
        Some(EnvironmentScope::OrchestrationOperate),
    )?;
    let req: ModelIdBody = parse_json_body(&body)?;
    let mut guard = state
        .voice_state
        .lock()
        .map_err(|_| EnvErr(Box::new(env_internal("voice state lock poisoned"))))?;
    guard.installed_models.remove(&req.model_id);
    Ok(Json(json!({ "models": guard.catalog() })).into_response())
}

/// `POST /api/voice/models/link`
pub(crate) async fn handle_voice_models_link(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> EnvResult<Response> {
    authorize_env(
        &state,
        &headers,
        Some(EnvironmentScope::OrchestrationOperate),
    )?;
    let req: ModelLinkBody = parse_json_body(&body)?;
    let path = PathBuf::from(req.path.trim());
    if !path.exists() {
        return Err(EnvErr(Box::new(env_invalid_request(
            "invalid_command",
            format!("Model path `{}` does not exist", path.display()),
        ))));
    }
    if path.is_file()
        && let Err(err) = inspect_model_file(&path)
    {
        return Err(EnvErr(Box::new(env_invalid_request(
            "invalid_command",
            err.to_string(),
        ))));
    }
    let mut guard = state
        .voice_state
        .lock()
        .map_err(|_| EnvErr(Box::new(env_internal("voice state lock poisoned"))))?;
    guard.mark_default_linked(path);
    Ok(Json(json!({ "models": guard.catalog() })).into_response())
}

/// `POST /api/voice/acceleration`
pub(crate) async fn handle_voice_acceleration(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> EnvResult<Response> {
    authorize_env(&state, &headers, Some(EnvironmentScope::SettingsWrite))?;
    let req: AccelerationBody = parse_json_body(&body)?;
    if req.acceleration != "auto"
        && req.acceleration != "cpu"
        && !req.acceleration.starts_with("gpu:")
    {
        return Err(EnvErr(Box::new(env_invalid_request(
            "invalid_command",
            "Invalid speech acceleration setting",
        ))));
    }
    let mut guard = state
        .voice_state
        .lock()
        .map_err(|_| EnvErr(Box::new(env_internal("voice state lock poisoned"))))?;
    guard.acceleration = req.acceleration;
    Ok(Json(guard.status_json()).into_response())
}

/// `POST /api/voice/model-unload-timeout`
pub(crate) async fn handle_voice_unload_timeout(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> EnvResult<Response> {
    authorize_env(&state, &headers, Some(EnvironmentScope::SettingsWrite))?;
    let req: UnloadTimeoutBody = parse_json_body(&body)?;
    if !matches!(
        req.timeout.as_str(),
        "never" | "immediately" | "min_2" | "min_5" | "min_10" | "min_15" | "hour_1"
    ) {
        return Err(EnvErr(Box::new(env_invalid_request(
            "invalid_command",
            "Invalid modelUnloadTimeout setting",
        ))));
    }
    let mut guard = state
        .voice_state
        .lock()
        .map_err(|_| EnvErr(Box::new(env_internal("voice state lock poisoned"))))?;
    guard.model_unload_timeout = req.timeout;
    Ok(Json(guard.status_json()).into_response())
}
