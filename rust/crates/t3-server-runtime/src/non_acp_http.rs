//! Live HTTP/SSE turn streaming for `OpenCodeHttp` and `GrokHttp`.

use futures_util::StreamExt;
use serde_json::Value;
use t3_contracts::ids::{
    CheckpointScopeId, ProviderDriverKind, ProviderSessionId, ProviderThreadId,
};
use t3_contracts::orchestration::{Run, RunAttempt};
use t3_orchestrator::sink::EventSink;
use t3_providers::non_acp::grok_http::{
    GrokChatMessage, build_grok_chat_completions_request, parse_grok_sse_chunk,
};
use t3_providers::non_acp::opencode_sse::{
    build_opencode_create_session_request, build_opencode_event_stream_request,
    build_opencode_prompt_request, parse_opencode_sse_data,
};
use t3_providers::non_acp::sse::SseStreamParser;
use t3_providers::non_acp::{NonAcpHttpRequestSpec, NonAcpParseError, NonAcpTurnUpdate};

use crate::error::ServerRuntimeError;
use crate::non_acp_turn_sink::{
    NonAcpStreamSummary, apply_non_acp_update, commit_http_turn_finish,
};
use crate::turn_events::{TurnEmitContext, commit_runtime_events};

/// HTTP streaming target configuration (`OpenCodeHttp` or `GrokHttp`).
#[derive(Debug, Clone)]
pub enum NonAcpHttpDriver {
    /// `OpenCode` HTTP session + SSE event stream.
    OpenCode {
        /// Base URL of the `OpenCode` server.
        base_url: String,
        /// Optional pre-formatted `Authorization` header (e.g. `"Basic ..."` or `"Bearer ..."`).
        auth_header: Option<String>,
        /// Optional model override.
        model: Option<String>,
    },
    /// Grok / xAI OpenAI-compatible SSE `/chat/completions` streaming endpoint.
    Grok {
        /// Endpoint base URL (`""` uses default xAI API base URL).
        endpoint_url: String,
        /// API key for `Authorization: Bearer`.
        api_key: String,
        /// Optional model override.
        model: Option<String>,
    },
}

/// Inputs for executing a streaming HTTP/SSE turn.
#[derive(Debug, Clone)]
pub struct NonAcpHttpTurnInput {
    /// Run record.
    pub run: Run,
    /// Attempt record if present.
    pub attempt: Option<RunAttempt>,
    /// Prompt text.
    pub prompt: String,
    /// Existing provider thread ID if bound.
    pub provider_thread_id: Option<ProviderThreadId>,
    /// ISO-8601 timestamp.
    pub now_iso: String,
    /// Checkpoint scope identifier.
    pub scope_id: CheckpointScopeId,
    /// Next timeline item ordinal.
    pub next_item_ordinal: u64,
    /// Whether to queue `ThreadTitleGenerate`.
    pub queue_title_generate: bool,
}

struct HttpStreamState<'a> {
    client: &'a reqwest::Client,
    sink: &'a EventSink,
    input: &'a NonAcpHttpTurnInput,
    ctx: &'a mut TurnEmitContext,
    summary: &'a mut NonAcpStreamSummary,
}

/// Execute one live HTTP/SSE streaming turn (`OpenCodeHttp` or `GrokHttp`) into `Orchestrator`.
pub async fn execute_non_acp_http_turn(
    sink: &EventSink,
    driver_cfg: &NonAcpHttpDriver,
    input: &NonAcpHttpTurnInput,
) -> Result<(), ServerRuntimeError> {
    let client = reqwest::Client::new();
    let driver = ProviderDriverKind::new("grok")
        .map_err(|e| ServerRuntimeError::Persistence(e.to_string()))?;
    let session_id = ProviderSessionId::new(format!("psess-{}", input.run.id.as_str()))
        .map_err(|e| ServerRuntimeError::Persistence(e.to_string()))?;
    let p_thread_id = match input.provider_thread_id.clone() {
        Some(existing) => existing,
        None => ProviderThreadId::new(format!("pthread-{}", input.run.thread_id.as_str()))
            .map_err(|e| ServerRuntimeError::Persistence(e.to_string()))?,
    };
    let mut ctx = TurnEmitContext::new(
        &input.run,
        driver,
        session_id,
        p_thread_id,
        input.now_iso.clone(),
        input.next_item_ordinal,
    )?;
    let start_events =
        ctx.build_start_events(&input.run, input.attempt.as_ref(), &input.now_iso)?;
    commit_runtime_events(
        sink,
        &input.run.thread_id,
        "runtime.http-provider.turn-started",
        &input.now_iso,
        start_events,
        Vec::new(),
    )?;

    let mut summary = NonAcpStreamSummary::default();
    let mut state = HttpStreamState {
        client: &client,
        sink,
        input,
        ctx: &mut ctx,
        summary: &mut summary,
    };
    let res = match driver_cfg {
        NonAcpHttpDriver::OpenCode {
            base_url,
            auth_header,
            model,
        } => {
            run_opencode_http(
                &mut state,
                base_url,
                auth_header.as_deref(),
                model.as_deref(),
            )
            .await
        }
        NonAcpHttpDriver::Grok {
            endpoint_url,
            api_key,
            model,
        } => run_grok_http(&mut state, endpoint_url, api_key, model.as_deref()).await,
    };

    commit_http_turn_finish(sink, input, &mut ctx, &summary, res)
}

async fn send_spec(
    client: &reqwest::Client,
    spec: NonAcpHttpRequestSpec,
    auth_override: Option<&str>,
) -> Result<reqwest::Response, ServerRuntimeError> {
    let method = spec
        .method
        .parse::<reqwest::Method>()
        .map_err(|e| ServerRuntimeError::BadRequest(e.to_string()))?;
    let mut req = client.request(method, &spec.url);
    for (k, v) in &spec.headers {
        req = req.header(k.as_str(), v.as_str());
    }
    if let Some(auth) = auth_override {
        req = req.header("Authorization", auth);
    }
    if let Some(body) = spec.body {
        req = req.json(&body);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| ServerRuntimeError::Upstream(e.to_string()))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(ServerRuntimeError::Upstream(format!(
            "HTTP {status} from {}: {text}",
            spec.url
        )));
    }
    Ok(resp)
}

async fn run_opencode_http(
    state: &mut HttpStreamState<'_>,
    base_url: &str,
    auth_header: Option<&str>,
    model_override: Option<&str>,
) -> Result<(), ServerRuntimeError> {
    let create_spec =
        build_opencode_create_session_request(base_url, None, None, Some("T3 Code Session"));
    let create_resp = send_spec(state.client, create_spec, auth_header).await?;
    let created: Value = create_resp
        .json()
        .await
        .map_err(|e| ServerRuntimeError::Upstream(e.to_string()))?;
    let session_id = created
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| ServerRuntimeError::Upstream("opencode response missing `id`".to_owned()))?
        .to_owned();

    let model_name = model_override.unwrap_or(&state.input.run.model_selection.model);
    let model_pair = model_name.split_once('/');
    let sse_spec = build_opencode_event_stream_request(base_url, None, None);
    let sse_resp = send_spec(state.client, sse_spec, auth_header).await?;

    let prompt_spec =
        build_opencode_prompt_request(base_url, None, &session_id, &state.input.prompt, model_pair);
    let _ = send_spec(state.client, prompt_spec, auth_header).await?;
    consume_sse_response(state, sse_resp, parse_opencode_sse_data, model_name).await
}

async fn run_grok_http(
    state: &mut HttpStreamState<'_>,
    endpoint_url: &str,
    api_key: &str,
    model_override: Option<&str>,
) -> Result<(), ServerRuntimeError> {
    let model = model_override.unwrap_or(&state.input.run.model_selection.model);
    let base = (!endpoint_url.trim().is_empty()).then_some(endpoint_url);
    let spec = build_grok_chat_completions_request(
        base,
        api_key,
        model,
        &[GrokChatMessage {
            role: "user".to_owned(),
            content: state.input.prompt.clone(),
        }],
        true,
        None,
    );
    let resp = send_spec(state.client, spec, None).await?;
    consume_sse_response(state, resp, parse_grok_sse_chunk, model).await
}

async fn consume_sse_response<F>(
    state: &mut HttpStreamState<'_>,
    resp: reqwest::Response,
    parse_fn: F,
    model_name: &str,
) -> Result<(), ServerRuntimeError>
where
    F: Fn(&str) -> Result<Vec<NonAcpTurnUpdate>, NonAcpParseError>,
{
    let mut parser = SseStreamParser::new();
    let mut byte_stream = resp.bytes_stream();
    while let Some(chunk_res) = byte_stream.next().await {
        let chunk = chunk_res.map_err(|e| ServerRuntimeError::Upstream(e.to_string()))?;
        let frames = parser
            .push_bytes(&chunk)
            .map_err(|e| ServerRuntimeError::Upstream(e.to_string()))?;
        for frame in frames {
            if frame.is_done {
                return Ok(());
            }
            let updates =
                parse_fn(&frame.data).map_err(|e| ServerRuntimeError::Upstream(e.to_string()))?;
            emit_http_updates(state, updates, model_name)?;
            if state.summary.terminal_seen {
                return Ok(());
            }
        }
    }
    let tail_frames = parser
        .finish()
        .map_err(|e| ServerRuntimeError::Upstream(e.to_string()))?;
    for frame in tail_frames {
        if frame.is_done {
            return Ok(());
        }
        let updates =
            parse_fn(&frame.data).map_err(|e| ServerRuntimeError::Upstream(e.to_string()))?;
        emit_http_updates(state, updates, model_name)?;
    }
    Ok(())
}

fn emit_http_updates(
    state: &mut HttpStreamState<'_>,
    updates: Vec<NonAcpTurnUpdate>,
    model_name: &str,
) -> Result<(), ServerRuntimeError> {
    let mut batch = Vec::new();
    for upd in updates {
        batch.extend(apply_non_acp_update(
            state.ctx,
            state.summary,
            upd,
            &state.input.now_iso,
            model_name,
        )?);
    }
    if !batch.is_empty() {
        commit_runtime_events(
            state.sink,
            &state.input.run.thread_id,
            "runtime.http-provider.stream",
            &state.input.now_iso,
            batch,
            Vec::new(),
        )?;
    }
    Ok(())
}
