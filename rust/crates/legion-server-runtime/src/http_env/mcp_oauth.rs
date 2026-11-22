//! `EnvironmentMcpOAuthHttpApi` metadata, dynamic client registration, authorize, and approval handlers.

use std::fmt::Write as _;
use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::header::LOCATION;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use legion_persistence::pairing_tokens::ADMINISTRATIVE_CLIENT_SCOPES;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::http_api::RuntimeHttpState;
use crate::http_auth::authenticate_headers;
use crate::http_env::{
    EnvErr, EnvResult, env_error_json, parse_json_body, resolve_request_origin,
    sign_b64url_payload, verify_b64url_payload,
};

pub(crate) fn mcp_approval_error(message: impl Into<String>) -> Response {
    env_error_json(
        StatusCode::BAD_REQUEST,
        json!({
            "_tag": "AuthMcpApprovalError",
            "message": message.into(),
        }),
    )
}

fn oauth_reg_error(error: &str, description: impl Into<String>) -> Response {
    env_error_json(
        StatusCode::BAD_REQUEST,
        json!({
            "error": error,
            "error_description": description.into(),
        }),
    )
}

fn is_valid_redirect_uri(uri: &str) -> bool {
    let trimmed = uri.trim();
    if trimmed.is_empty() || trimmed.contains('#') {
        return false;
    }
    if trimmed.starts_with("http://localhost")
        || trimmed.starts_with("http://127.0.0.1")
        || trimmed.starts_with("http://[::1]")
        || trimmed.starts_with("https://")
    {
        return true;
    }
    trimmed.split_once("://").is_some_and(|(scheme, _)| {
        !scheme.is_empty()
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '+')
    })
}

fn extract_host_from_uri(uri: &str) -> String {
    uri.split_once("://")
        .map_or(uri, |(_, rest)| rest.split('/').next().unwrap_or(rest))
        .to_owned()
}

pub(crate) fn percent_encode_component(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// `GET /.well-known/oauth-protected-resource` and `/.well-known/oauth-protected-resource/mcp`.
pub(crate) async fn handle_mcp_protected_resource(headers: HeaderMap) -> Response {
    let origin = resolve_request_origin(&headers);
    Json(json!({
        "resource": format!("{origin}/mcp"),
        "authorization_servers": [origin],
        "scopes_supported": ADMINISTRATIVE_CLIENT_SCOPES,
        "bearer_methods_supported": ["header"],
        "resource_name": "LegionCode",
    }))
    .into_response()
}

/// `GET /.well-known/oauth-authorization-server`
pub(crate) async fn handle_mcp_authorization_server(headers: HeaderMap) -> Response {
    let origin = resolve_request_origin(&headers);
    Json(json!({
        "issuer": origin,
        "authorization_endpoint": format!("{origin}/oauth/mcp/authorize"),
        "token_endpoint": format!("{origin}/oauth/mcp/token"),
        "registration_endpoint": format!("{origin}/oauth/mcp/register"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
        "scopes_supported": ADMINISTRATIVE_CLIENT_SCOPES,
        "authorization_response_iss_parameter_supported": true,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct RegisterClientBody {
    client_name: Option<String>,
    redirect_uris: Option<Vec<String>>,
    token_endpoint_auth_method: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SignedMcpClient {
    pub client_name: String,
    pub redirect_uris: Vec<String>,
}

/// `POST /oauth/mcp/register` — stateless HMAC-signed dynamic client registration.
pub(crate) async fn handle_mcp_register(
    State(state): State<Arc<RuntimeHttpState>>,
    body: Bytes,
) -> Response {
    let req: RegisterClientBody = match parse_json_body(&body) {
        Ok(v) => v,
        Err(_) => {
            return oauth_reg_error("invalid_client_metadata", "Invalid registration JSON body");
        }
    };
    if let Some(ref auth_method) = req.token_endpoint_auth_method
        && auth_method != "none"
    {
        return oauth_reg_error(
            "invalid_client_metadata",
            "Only token_endpoint_auth_method=\"none\" is supported",
        );
    }
    let redirect_uris = req.redirect_uris.unwrap_or_default();
    if redirect_uris.is_empty() || !redirect_uris.iter().all(|u| is_valid_redirect_uri(u)) {
        return oauth_reg_error(
            "invalid_redirect_uri",
            "At least one valid redirect_uri is required",
        );
    }
    let client_name = req
        .client_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("MCP Client")
        .to_owned();
    let payload = SignedMcpClient {
        client_name: client_name.clone(),
        redirect_uris: redirect_uris.clone(),
    };
    let client_id = format!(
        "mcp_c_{}",
        sign_b64url_payload(
            &state.signed_asset_secret,
            &serde_json::to_vec(&payload).unwrap_or_default(),
        )
    );

    (
        StatusCode::CREATED,
        Json(json!({
            "client_id": client_id,
            "client_name": client_name,
            "redirect_uris": redirect_uris,
            "grant_types": ["authorization_code"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        })),
    )
        .into_response()
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct McpAuthorizeParams {
    pub response_type: Option<String>,
    pub client_id: Option<String>,
    pub redirect_uri: Option<String>,
    pub code_challenge: Option<String>,
    pub code_challenge_method: Option<String>,
    pub state: Option<String>,
    pub resource: Option<String>,
}

fn verify_mcp_client(state: &RuntimeHttpState, client_id: &str) -> EnvResult<SignedMcpClient> {
    let encoded = client_id.strip_prefix("mcp_c_").ok_or_else(|| {
        EnvErr(Box::new(mcp_approval_error(
            "Unrecognized MCP OAuth client_id",
        )))
    })?;
    let bytes = verify_b64url_payload(&state.signed_asset_secret, encoded).ok_or_else(|| {
        EnvErr(Box::new(mcp_approval_error(
            "Invalid MCP OAuth client_id signature",
        )))
    })?;
    serde_json::from_slice(&bytes).map_err(|_| {
        EnvErr(Box::new(mcp_approval_error(
            "Corrupt MCP OAuth client_id payload",
        )))
    })
}

pub(crate) fn validate_authorize_params(
    state: &RuntimeHttpState,
    params: &McpAuthorizeParams,
) -> EnvResult<(SignedMcpClient, String, String)> {
    if params.response_type.as_deref() != Some("code") {
        return Err(EnvErr(Box::new(mcp_approval_error(
            "response_type must be `code`",
        ))));
    }
    if params.code_challenge_method.as_deref() != Some("S256") {
        return Err(EnvErr(Box::new(mcp_approval_error(
            "code_challenge_method must be `S256`",
        ))));
    }
    let challenge = params
        .code_challenge
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| EnvErr(Box::new(mcp_approval_error("code_challenge is required"))))?
        .to_owned();
    let client_id = params
        .client_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| EnvErr(Box::new(mcp_approval_error("client_id is required"))))?;
    let client = verify_mcp_client(state, client_id)?;
    let redirect_uri = params
        .redirect_uri
        .as_deref()
        .map(str::trim)
        .filter(|s| client.redirect_uris.iter().any(|allowed| allowed == *s))
        .ok_or_else(|| {
            EnvErr(Box::new(mcp_approval_error(
                "redirect_uri is not registered for this client",
            )))
        })?
        .to_owned();
    Ok((client, redirect_uri, challenge))
}

/// `GET /oauth/mcp/authorize` — validates OAuth PKCE params and redirects to web approval UI.
pub(crate) async fn handle_mcp_authorize(
    State(state): State<Arc<RuntimeHttpState>>,
    Query(params): Query<McpAuthorizeParams>,
) -> EnvResult<Response> {
    validate_authorize_params(&state, &params)?;
    let web_base = state.dev_url.as_deref().unwrap_or("");
    let mut query_parts = vec![
        format!(
            "response_type={}",
            percent_encode_component(params.response_type.as_deref().unwrap_or("code"))
        ),
        format!(
            "client_id={}",
            percent_encode_component(params.client_id.as_deref().unwrap_or_default())
        ),
        format!(
            "redirect_uri={}",
            percent_encode_component(params.redirect_uri.as_deref().unwrap_or_default())
        ),
        format!(
            "code_challenge={}",
            percent_encode_component(params.code_challenge.as_deref().unwrap_or_default())
        ),
        format!(
            "code_challenge_method={}",
            percent_encode_component(params.code_challenge_method.as_deref().unwrap_or("S256"))
        ),
    ];
    if let Some(ref st) = params.state {
        query_parts.push(format!("state={}", percent_encode_component(st)));
    }
    if let Some(ref res) = params.resource {
        query_parts.push(format!("resource={}", percent_encode_component(res)));
    }
    let dest = format!("{web_base}/connect/mcp?{}", query_parts.join("&"));
    let mut resp = StatusCode::FOUND.into_response();
    if let Ok(hv) = HeaderValue::from_str(&dest) {
        resp.headers_mut().insert(LOCATION, hv);
    }
    Ok(resp)
}

/// `POST /oauth/mcp/approval` — inspects approval request details and returns CSRF / one-click options.
pub(crate) async fn handle_mcp_approval(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> EnvResult<Response> {
    let params: McpAuthorizeParams = parse_json_body(&body).map_err(|_| {
        EnvErr(Box::new(mcp_approval_error(
            "Invalid authorization request payload",
        )))
    })?;
    let (client, redirect_uri, challenge) = validate_authorize_params(&state, &params)?;
    let origin = resolve_request_origin(&headers);
    let environment_host = extract_host_from_uri(&origin);
    let redirect_host = extract_host_from_uri(&redirect_uri);

    let principal = authenticate_headers(&state, &headers).ok();
    let mut obj = json!({
        "clientName": client.client_name,
        "redirectHost": redirect_host,
        "environmentHost": environment_host,
    });
    if let Some(p) = principal
        && p.scopes.iter().any(|s| s == "access:write" || s == "*")
    {
        let csrf_token = sign_b64url_payload(
            &state.signed_asset_secret,
            format!("{}|{}", p.session_id, challenge).as_bytes(),
        );
        if let Value::Object(ref mut map) = obj {
            map.insert("csrfToken".to_owned(), Value::String(csrf_token));
            map.insert(
                "oneClickAccess".to_owned(),
                json!([
                    "read-only",
                    "approval-required",
                    "auto-accept-edits",
                    "auto",
                    "full-access"
                ]),
            );
        }
    }
    Ok(Json(obj).into_response())
}
