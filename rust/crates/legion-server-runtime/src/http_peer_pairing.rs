//! Transitive peer & client pairing HTTP handlers (`#40036`) wired to `legion_remote::peer::client_pairing`.

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::header::{AUTHORIZATION, ORIGIN};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use legion_contracts::environment::ExecutionEnvironmentDescriptor;
use legion_contracts::ids::EnvironmentId;
use legion_contracts::orchestration::McpClientAccess;
use legion_contracts::peer_links::{
    PeerLinkClientPairingRequest, PeerLinkClientPairingResult, PeerLinkLinkPairingInput,
    PeerMcpClientAccess,
};
use legion_contracts::permissions::EnvironmentScope;
use legion_persistence::ownership::format_epoch_millis_utc;
use legion_persistence::pairing_tokens::{
    IssuePairingTokenInput, VerifiedBearerSession, issue_pairing_token_with_conn,
    verify_bearer_session_with_conn,
};
use legion_remote::peer::client_pairing::{
    ApprovedPairingIssue, CLIENT_PAIRING_TTL_MS, PairingIssuerCaller, PeerPairingClient,
    evaluate_pairing_issue, link_client_name_prefix,
};
use legion_remote::peer::links::PeerLinksService;
use serde::Deserialize;
use serde_json::json;

use crate::error::ServerRuntimeError;
use crate::http_api::RuntimeHttpState;
use crate::http_auth::{constant_time_eq, now_unix_ms};
use crate::http_auth_guard::parse_authorized_json;
use crate::outbox_state::utc_now_iso;

const STANDARD_DELEGABLE_SCOPES: &[&str] = &[
    "orchestration:read",
    "orchestration:operate",
    "terminal:read",
    "terminal:operate",
    "filesystem:read",
    "preview:operate",
];

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemotePeerPairRequest {
    environment_id: EnvironmentId,
    #[serde(default)]
    access: Option<PeerMcpClientAccess>,
    #[serde(default)]
    client_label: Option<String>,
    #[serde(default)]
    scopes: Vec<String>,
    capabilities: ExecutionEnvironmentDescriptor,
    peer_url: Option<String>,
}

/// `POST /api/peer-link/client-pairing` (and `/api/links/client-pairing`) — issues a local
/// credential for a transitive client or link (`#40036`).
pub(crate) async fn handle_issue_client_pairing(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ServerRuntimeError> {
    if headers.get(ORIGIN).is_some() {
        return Err(ServerRuntimeError::Forbidden(
            "Origin header is not allowed".to_owned(),
        ));
    }

    let session = authenticate_peer_bearer(&state, &headers)?;
    let req: PeerLinkClientPairingRequest = serde_json::from_slice(&body)
        .map_err(|err| ServerRuntimeError::BadRequest(format!("invalid JSON body: {err}")))?;

    let caller = derive_peer_link_caller(&session);
    let max_runtime_mode = state
        .managed_policy
        .as_ref()
        .and_then(|p| p.limits.as_ref())
        .and_then(|l| l.max_runtime_mode);
    let approved = evaluate_pairing_issue(
        state.linked_environments_enabled,
        max_runtime_mode,
        &caller,
        &req,
        STANDARD_DELEGABLE_SCOPES,
    )
    .map_err(|refused| ServerRuntimeError::Forbidden(refused.message))?;

    let (scopes, label, ttl_ms) = match approved {
        ApprovedPairingIssue::PeerLink { access, label } => {
            let s = match access {
                PeerMcpClientAccess::FullAccess => vec![
                    EnvironmentScope::OrchestrationRead.as_str().to_owned(),
                    EnvironmentScope::OrchestrationOperate.as_str().to_owned(),
                    EnvironmentScope::SettingsWrite.as_str().to_owned(),
                    "source-control:write".to_owned(),
                    EnvironmentScope::AccessRead.as_str().to_owned(),
                    EnvironmentScope::AccessWrite.as_str().to_owned(),
                ],
                PeerMcpClientAccess::Auto | PeerMcpClientAccess::ApprovalRequired => vec![
                    EnvironmentScope::OrchestrationRead.as_str().to_owned(),
                    EnvironmentScope::OrchestrationOperate.as_str().to_owned(),
                ],
                PeerMcpClientAccess::ReadOnly => {
                    vec![EnvironmentScope::OrchestrationRead.as_str().to_owned()]
                }
            };
            (s, label, 10 * 60 * 1_000)
        }
        ApprovedPairingIssue::Client {
            scopes,
            label,
            ttl_ms,
        } => (
            scopes,
            label,
            i64::try_from(ttl_ms)
                .unwrap_or(i64::try_from(CLIENT_PAIRING_TTL_MS).unwrap_or(180_000)),
        ),
    };

    let now_ms = now_unix_ms();
    let conn = state
        .db()
        .lock()
        .map_err(|_| ServerRuntimeError::Persistence("db mutex poisoned".to_owned()))?;
    let issued = issue_pairing_token_with_conn(
        &conn,
        &IssuePairingTokenInput {
            scopes,
            ttl_ms,
            label: Some(label),
        },
        now_ms,
    )?;

    Ok((
        StatusCode::OK,
        Json(json!({
            "credential": issued.credential,
            "expiresAt": format_epoch_millis_utc(issued.expires_at_ms),
            "expiresAtMs": issued.expires_at_ms,
        })),
    )
        .into_response())
}

fn authenticate_peer_bearer(
    state: &RuntimeHttpState,
    headers: &HeaderMap,
) -> Result<VerifiedBearerSession, ServerRuntimeError> {
    let token = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|raw| {
            let mut parts = raw.splitn(2, char::is_whitespace);
            let scheme = parts.next()?.trim();
            let rest = parts.next()?.trim();
            (scheme.eq_ignore_ascii_case("bearer") && !rest.is_empty()).then_some(rest)
        })
        .ok_or_else(|| ServerRuntimeError::Unauthorized("missing bearer token".to_owned()))?;

    if let Some(ref dev_tok) = state.dev_auth_token
        && constant_time_eq(token.as_bytes(), dev_tok.as_bytes())
    {
        return Err(ServerRuntimeError::Unauthorized(
            "dev token cannot be used for peer client pairing".to_owned(),
        ));
    }

    let now_ms = now_unix_ms();
    let conn = state
        .db()
        .lock()
        .map_err(|_| ServerRuntimeError::Persistence("db mutex poisoned".to_owned()))?;
    let session = verify_bearer_session_with_conn(&conn, token, now_ms)?;
    if session.method != "bearer-access-token" {
        return Err(ServerRuntimeError::Unauthorized(
            "only bearer access token sessions are accepted".to_owned(),
        ));
    }
    Ok(session)
}

fn derive_peer_link_caller(session: &VerifiedBearerSession) -> PairingIssuerCaller {
    let raw_label = session
        .client_label
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Peer link");
    let brand_prefix = link_client_name_prefix();
    let has_link_prefix =
        raw_label.starts_with(&brand_prefix) || raw_label.starts_with("T3 Code · ");
    let has_peer_subject =
        session.subject == "peer-link" || session.subject.starts_with("peer-link:");
    let is_peer_link = has_peer_subject
        || (matches!(session.subject.as_str(), "mcp-client" | "mcp-oauth") && has_link_prefix);
    let access = derive_peer_link_access(&session.subject, &session.scopes);
    let label = raw_label
        .strip_prefix("T3 Code · ")
        .unwrap_or(raw_label)
        .to_owned();
    PairingIssuerCaller {
        is_peer_link,
        access,
        label,
    }
}

fn derive_peer_link_access(subject: &str, scopes: &[String]) -> McpClientAccess {
    let has_wildcard = scopes.iter().any(|s| s == "*");
    let has_operate = has_wildcard
        || scopes
            .iter()
            .any(|s| s == EnvironmentScope::OrchestrationOperate.as_str());
    let has_full_scopes = has_wildcard
        || (has_operate
            && (scopes
                .iter()
                .any(|s| s == EnvironmentScope::AccessWrite.as_str())
                || (scopes
                    .iter()
                    .any(|s| s == EnvironmentScope::SettingsWrite.as_str())
                    && scopes
                        .iter()
                        .any(|s| s == "source-control:write" || s == "source_control:write"))));
    let scope_tier = if has_full_scopes {
        McpClientAccess::FullAccess
    } else if has_operate {
        McpClientAccess::ApprovalRequired
    } else {
        McpClientAccess::ReadOnly
    };
    match subject.strip_prefix("peer-link:") {
        Some("read-only") => McpClientAccess::ReadOnly,
        Some("approval-required") => McpClientAccess::ApprovalRequired,
        Some("auto-accept-edits") => McpClientAccess::AutoAcceptEdits,
        Some("auto") => McpClientAccess::Auto,
        Some("full-access") | None => scope_tier,
        Some(_) => {
            if has_operate {
                McpClientAccess::ApprovalRequired
            } else {
                McpClientAccess::ReadOnly
            }
        }
    }
}

/// `POST /api/peers/pair-link` — asks an already-linked peer for a transitive pairing code (`#40036`).
pub(crate) async fn handle_pair_link_through_peer(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ServerRuntimeError> {
    let (principal, req): (_, RemotePeerPairRequest) = parse_authorized_json(
        &state,
        &headers,
        &[EnvironmentScope::AccessWrite],
        true,
        &body,
    )?;

    let client_label = req
        .client_label
        .unwrap_or_else(|| format!("session:{}", principal.session_id));
    let secrets = state.secrets_store()?;
    let creds = {
        let conn = state
            .db()
            .lock()
            .map_err(|_| ServerRuntimeError::Persistence("db mutex poisoned".to_owned()))?;
        PeerLinksService::new(secrets)
            .resolve_credentials(&conn, req.environment_id.as_str(), &utc_now_iso())
            .map_err(|err| ServerRuntimeError::Conflict(err.message))?
    };

    let peer_url = req
        .peer_url
        .or_else(|| creds.link.urls.first().cloned())
        .ok_or_else(|| {
            ServerRuntimeError::BadRequest("Linked peer has no reachable URL".to_owned())
        })?;

    let pairing_client =
        PeerPairingClient::new().map_err(|err| ServerRuntimeError::Upstream(err.message))?;

    let result: PeerLinkClientPairingResult = if let Some(access) = req.access {
        pairing_client
            .request_link_pairing(
                &creds.link,
                &req.capabilities.capabilities,
                &peer_url,
                &creds.token,
                &client_label,
                &PeerLinkLinkPairingInput {
                    environment_id: req.environment_id,
                    access,
                },
            )
            .await
            .map_err(|err| ServerRuntimeError::Upstream(err.message))?
    } else {
        pairing_client
            .request_pairing(
                &creds.link,
                &req.capabilities.capabilities,
                &peer_url,
                &creds.token,
                PeerLinkClientPairingRequest {
                    scopes: req.scopes,
                    client_label,
                    link: None,
                },
            )
            .await
            .map_err(|err| ServerRuntimeError::Upstream(err.message))?
    };

    Ok((StatusCode::OK, Json(result)).into_response())
}
