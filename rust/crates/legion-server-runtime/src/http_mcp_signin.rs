//! MCP OAuth server sign-in state and HTTP endpoints (`#39921`).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex as StdMutex};

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use legion_contracts::ids::{ProviderThreadId, ThreadId};
use legion_contracts::mcp_sign_in::{
    OrchestrationMcpSignInNotice, ProviderMcpSignInCompleteInput, ProviderMcpSignInCompleteResult,
    ProviderMcpSignInError, ProviderMcpSignInStartInput, ProviderMcpSignInStartResult,
};
use legion_contracts::permissions::EnvironmentScope;
use legion_shared::oauth_callback::oauth_callback_url;
use serde_json::json;

use crate::error::ServerRuntimeError;
use crate::http_api::RuntimeHttpState;
use crate::http_auth_guard::parse_authorized_json;
use crate::outbox_state::utc_now_iso;

type SignInKey = (String, String, String);

/// Registered pending/completed OAuth flow state for one provider thread MCP server.
#[derive(Debug, Clone)]
pub struct McpOAuthFlowConfig {
    /// Redirect URI expected on loopback OAuth callback completion.
    pub redirect_uri: String,
    /// Authorization URL presented to the client (unless already signed in).
    pub authorization_url: Option<String>,
    /// Expected `state` parameter on callback completion.
    pub expected_state: String,
    /// Expected `client_id` parameter if verified during callback completion.
    pub expected_client_id: Option<String>,
}

/// Callback flow record persisted in memory for active MCP OAuth sign-in requests.
#[derive(Debug, Clone)]
struct McpSignInEntry {
    flow: McpOAuthFlowConfig,
    signed_in_at: Option<String>,
}

/// Runtime registry backing `/api/mcp/sign-in/start` and `/api/mcp/sign-in/complete` (`#39921`).
#[derive(Debug, Default)]
pub struct McpSignInRegistry {
    entries: StdMutex<BTreeMap<SignInKey, McpSignInEntry>>,
}

impl McpSignInRegistry {
    /// Create an empty [`McpSignInRegistry`].
    pub fn new() -> Self {
        Self::default()
    }

    fn key_for(
        thread_id: &ThreadId,
        provider_thread_id: &ProviderThreadId,
        server: &str,
    ) -> SignInKey {
        (
            thread_id.as_str().to_owned(),
            provider_thread_id.as_str().to_owned(),
            server.trim().to_owned(),
        )
    }

    /// Register or overwrite an OAuth flow definition for `(thread_id, provider_thread_id, server)`.
    pub fn register_flow(
        &self,
        thread_id: &ThreadId,
        provider_thread_id: &ProviderThreadId,
        server: &str,
        flow: McpOAuthFlowConfig,
    ) -> Result<(), ProviderMcpSignInError> {
        let server_name = server.trim();
        let mut guard = self.entries.lock().map_err(|_| ProviderMcpSignInError {
            server: server_name.to_owned(),
            detail: "MCP sign-in registry mutex poisoned".to_owned(),
        })?;
        guard.insert(
            Self::key_for(thread_id, provider_thread_id, server_name),
            McpSignInEntry {
                flow,
                signed_in_at: None,
            },
        );
        Ok(())
    }

    /// Start (or check completion status of) an MCP server OAuth sign-in flow.
    pub fn start(
        &self,
        input: &ProviderMcpSignInStartInput,
    ) -> Result<ProviderMcpSignInStartResult, ProviderMcpSignInError> {
        let server = input.server.trim();
        if server.is_empty() {
            return Err(ProviderMcpSignInError {
                server: input.server.clone(),
                detail: "MCP server name is required.".to_owned(),
            });
        }
        let guard = self.entries.lock().map_err(|_| ProviderMcpSignInError {
            server: server.to_owned(),
            detail: "MCP sign-in registry mutex poisoned".to_owned(),
        })?;
        let key = Self::key_for(&input.thread_id, &input.provider_thread_id, server);
        let Some(entry) = guard.get(&key) else {
            return Err(ProviderMcpSignInError {
                server: server.to_owned(),
                detail: format!("MCP server `{server}` does not have an active sign-in flow."),
            });
        };
        Ok(ProviderMcpSignInStartResult {
            authorization_url: if entry.signed_in_at.is_some() {
                None
            } else {
                entry.flow.authorization_url.clone()
            },
            signed_in_at: entry.signed_in_at.clone(),
        })
    }

    /// Validate an OAuth loopback callback URL and complete MCP server sign-in.
    pub fn complete(
        &self,
        input: &ProviderMcpSignInCompleteInput,
    ) -> Result<ProviderMcpSignInCompleteResult, ProviderMcpSignInError> {
        let server = input.server.trim();
        if server.is_empty() {
            return Err(ProviderMcpSignInError {
                server: input.server.clone(),
                detail: "MCP server name is required.".to_owned(),
            });
        }
        let mut guard = self.entries.lock().map_err(|_| ProviderMcpSignInError {
            server: server.to_owned(),
            detail: "MCP sign-in registry mutex poisoned".to_owned(),
        })?;
        let key = Self::key_for(&input.thread_id, &input.provider_thread_id, server);
        let Some(entry) = guard.get_mut(&key) else {
            return Err(ProviderMcpSignInError {
                server: server.to_owned(),
                detail: format!("MCP server `{server}` does not have an active sign-in flow."),
            });
        };

        let validated = oauth_callback_url(
            &input.callback_url,
            &entry.flow.redirect_uri,
            &entry.flow.expected_state,
        )
        .map_err(|err| ProviderMcpSignInError {
            server: server.to_owned(),
            detail: err.to_string(),
        })?;
        if let Some(ref expected_client_id) = entry.flow.expected_client_id
            && validated.client_id.as_deref() != Some(expected_client_id.as_str())
        {
            return Err(ProviderMcpSignInError {
                server: server.to_owned(),
                detail: "This redirect URL does not belong to the current sign-in.".to_owned(),
            });
        }

        if let Some(oauth_err) = validated.error {
            let desc = validated
                .error_description
                .map_or_default(|d| format!(": {d}"));
            return Err(ProviderMcpSignInError {
                server: server.to_owned(),
                detail: format!("OAuth authorization failed ({oauth_err}){desc}"),
            });
        }

        let signed_in_at = utc_now_iso();
        entry.signed_in_at = Some(signed_in_at.clone());
        Ok(ProviderMcpSignInCompleteResult { signed_in_at })
    }

    /// Build an orchestration notice payload for a pending MCP server sign-in (`#39921`).
    pub fn build_notice(server: &str) -> OrchestrationMcpSignInNotice {
        OrchestrationMcpSignInNotice {
            server: server.trim().to_owned(),
        }
    }
}

fn map_sign_in_error_response(err: ProviderMcpSignInError) -> Response {
    let status = if err.detail.contains("does not have an active") {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::BAD_REQUEST
    };
    (status, Json(err)).into_response()
}

/// `POST /api/mcp/sign-in/start` — begin or query an MCP server OAuth sign-in flow (`AuthProvidersManage`).
pub(crate) async fn handle_mcp_sign_in_start(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ServerRuntimeError> {
    let (_principal, input): (_, ProviderMcpSignInStartInput) = parse_authorized_json(
        &state,
        &headers,
        &[EnvironmentScope::ProvidersManage],
        true,
        &body,
    )?;

    match state.mcp_sign_in.start(&input) {
        Ok(res) => Ok((StatusCode::OK, Json(res)).into_response()),
        Err(err) => Ok(map_sign_in_error_response(err)),
    }
}

/// `POST /api/mcp/sign-in/complete` — validate OAuth callback redirect URL and complete sign-in (`AuthProvidersManage`).
pub(crate) async fn handle_mcp_sign_in_complete(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ServerRuntimeError> {
    let (_principal, input): (_, ProviderMcpSignInCompleteInput) = parse_authorized_json(
        &state,
        &headers,
        &[EnvironmentScope::ProvidersManage],
        true,
        &body,
    )?;

    match state.mcp_sign_in.complete(&input) {
        Ok(res) => Ok((StatusCode::OK, Json(res)).into_response()),
        Err(err) => Ok(map_sign_in_error_response(err)),
    }
}

/// Query or register an OAuth flow on HTTP (or return summary of active notices).
pub(crate) async fn handle_mcp_sign_in_notice(
    State(_state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ServerRuntimeError> {
    let server = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("server").and_then(|s| s.as_str()).map(str::to_owned))
        .unwrap_or_default();
    let _ = headers;
    if server.trim().is_empty() {
        return Err(ServerRuntimeError::BadRequest(
            "`server` field is required".to_owned(),
        ));
    }
    Ok((
        StatusCode::OK,
        Json(json!({ "notice": McpSignInRegistry::build_notice(&server) })),
    )
        .into_response())
}
