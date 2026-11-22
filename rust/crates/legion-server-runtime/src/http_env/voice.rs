//! `EnvironmentVoiceHttpApi` cloud-key, status, transcription, and post-process handlers.

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use legion_contracts::permissions::EnvironmentScope;
use legion_speech::{AcousticAsrEngine, AsrEngine, TranscriptionOptions, discover_speech_devices};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::http_api::RuntimeHttpState;
use crate::http_env::voice_state::{cloud_speech_error, valid_cloud_provider};
use crate::http_env::{
    EnvErr, EnvResult, authorize_env, env_bad_request, env_internal, env_invalid_request,
    parse_json_body,
};

const MAX_CLOUD_AUDIO_BYTES: usize = 25 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct SetCloudKeyBody {
    key: Option<Value>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct CloudTranscribeQuery {
    #[expect(dead_code, reason = "query param decoded for OpenAPI/schema parity")]
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PostProcessBody {
    text: String,
}

/// `GET /api/voice/cloud/keys`
pub(crate) async fn handle_voice_cloud_keys(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
) -> EnvResult<Response> {
    authorize_env(&state, &headers, Some(EnvironmentScope::OrchestrationRead))?;
    let guard = state
        .voice_state
        .lock()
        .map_err(|_| EnvErr(Box::new(env_internal("voice state lock poisoned"))))?;
    Ok(Json(json!({
        "openai": guard.openai_key.is_some(),
        "xai": guard.xai_key.is_some(),
        "openrouter": guard.openrouter_key.is_some(),
    }))
    .into_response())
}

/// `POST /api/voice/cloud/{provider}/key`
pub(crate) async fn handle_voice_set_cloud_key(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    Path(provider): Path<String>,
    body: Bytes,
) -> EnvResult<Response> {
    authorize_env(&state, &headers, Some(EnvironmentScope::ProvidersManage))?;
    let provider = valid_cloud_provider(provider.trim())?;
    let req: SetCloudKeyBody = parse_json_body(&body)?;
    let extracted = match req.key {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return Err(EnvErr(Box::new(env_bad_request(
                    "API key must not be empty",
                ))));
            }
            Some(trimmed.to_owned())
        }
        Some(Value::Object(map)) => map
            .get("name")
            .or_else(|| map.get("secret"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        Some(_) => return Err(EnvErr(Box::new(env_bad_request("Invalid key value")))),
    };

    let mut guard = state
        .voice_state
        .lock()
        .map_err(|_| EnvErr(Box::new(env_internal("voice state lock poisoned"))))?;
    match provider {
        "openai" => guard.openai_key = extracted,
        "xai" => guard.xai_key = extracted,
        "openrouter" => guard.openrouter_key = extracted,
        _ => {}
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `GET /api/voice/cloud/openrouter/models`
pub(crate) async fn handle_voice_cloud_openrouter_models(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
) -> EnvResult<Response> {
    authorize_env(&state, &headers, Some(EnvironmentScope::OrchestrationRead))?;
    let has_key = state
        .voice_state
        .lock()
        .is_ok_and(|g| g.openrouter_key.is_some());
    if !has_key {
        return Ok(cloud_speech_error(
            "openrouter",
            "missing-key",
            "OpenRouter API key is not configured",
        ));
    }
    Ok(Json(json!({
        "models": [
            { "id": "openai/gpt-4o-mini", "name": "GPT-4o Mini" },
            { "id": "anthropic/claude-haiku-4.5", "name": "Claude Haiku 4.5" }
        ]
    }))
    .into_response())
}

/// `POST /api/voice/cloud/{provider}/transcribe`
pub(crate) async fn handle_voice_cloud_transcribe(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    Path(provider): Path<String>,
    Query(_query): Query<CloudTranscribeQuery>,
    body: Bytes,
) -> EnvResult<Response> {
    authorize_env(
        &state,
        &headers,
        Some(EnvironmentScope::OrchestrationOperate),
    )?;
    let provider = valid_cloud_provider(provider.trim())?;
    if body.len() > MAX_CLOUD_AUDIO_BYTES {
        return Err(EnvErr(Box::new(env_invalid_request(
            "invalid_audio",
            "Audio payload exceeds maximum size",
        ))));
    }
    let has_key = state.voice_state.lock().is_ok_and(|g| match provider {
        "openai" => g.openai_key.is_some(),
        "xai" => g.xai_key.is_some(),
        "openrouter" => g.openrouter_key.is_some(),
        _ => false,
    });
    if !has_key {
        return Ok(cloud_speech_error(
            provider,
            "missing-key",
            format!("{provider} API key is not configured"),
        ));
    }
    Ok(cloud_speech_error(
        provider,
        "unavailable",
        format!("{provider} remote speech transcription endpoint is unavailable offline"),
    ))
}

/// `GET /api/voice/status`
pub(crate) async fn handle_voice_status(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
) -> EnvResult<Response> {
    authorize_env(&state, &headers, Some(EnvironmentScope::OrchestrationRead))?;
    let _devices = discover_speech_devices();
    let guard = state
        .voice_state
        .lock()
        .map_err(|_| EnvErr(Box::new(env_internal("voice state lock poisoned"))))?;
    Ok(Json(guard.status_json()).into_response())
}

/// `POST /api/voice/transcribe`
pub(crate) async fn handle_voice_transcribe(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> EnvResult<Response> {
    authorize_env(
        &state,
        &headers,
        Some(EnvironmentScope::OrchestrationOperate),
    )?;
    if body.is_empty() || body.len() > MAX_CLOUD_AUDIO_BYTES {
        return Err(EnvErr(Box::new(env_invalid_request(
            "invalid_audio",
            "Audio payload is empty or too large",
        ))));
    }
    let mut engine = AcousticAsrEngine::with_default_english();
    let text = if let Ok(samples) = legion_speech::decode_wav_to_16khz_mono(&body) {
        engine
            .transcribe_offline(&samples, &TranscriptionOptions::default())
            .unwrap_or_default()
    } else {
        String::new()
    };
    Ok(Json(json!({ "text": text })).into_response())
}

/// `POST /api/voice/post-process`
pub(crate) async fn handle_voice_post_process(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> EnvResult<Response> {
    authorize_env(
        &state,
        &headers,
        Some(EnvironmentScope::OrchestrationOperate),
    )?;
    let req: PostProcessBody = parse_json_body(&body)?;
    let cleaned = req.text.split_whitespace().collect::<Vec<_>>().join(" ");
    Ok(Json(json!({ "text": cleaned })).into_response())
}
