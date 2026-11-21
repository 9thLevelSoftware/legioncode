//! `BUG-04` request authentication & scope enforcement helper for Axum HTTP routes.

use axum::body::Bytes;
use axum::http::HeaderMap;
use serde::de::DeserializeOwned;
use serde_json::Value;
use t3_contracts::permissions::{EnvironmentScope, SessionGrantInput, grants_all_scopes};

use crate::error::ServerRuntimeError;
use crate::http_api::RuntimeHttpState;
use crate::http_auth::{AuthenticatedPrincipal, authenticate_headers};

/// Authenticates request headers (`BUG-03`) and verifies all `required` scopes (`BUG-04`).
pub fn authorize_request(
    state: &RuntimeHttpState,
    headers: &HeaderMap,
    required: &[EnvironmentScope],
    is_mutation: bool,
) -> Result<AuthenticatedPrincipal, ServerRuntimeError> {
    let principal = authenticate_headers(state, headers)?;
    authorize_scopes(&principal, required, is_mutation)?;
    Ok(principal)
}

/// Authorizes `BUG-03`/`BUG-04` BEFORE parsing JSON body so unauthorized/forbidden requests
/// never fall through to `422 Unprocessable Entity` payload validation errors.
pub(crate) fn parse_authorized_json<T: DeserializeOwned>(
    state: &RuntimeHttpState,
    headers: &HeaderMap,
    required: &[EnvironmentScope],
    is_mutation: bool,
    body: &Bytes,
) -> Result<(AuthenticatedPrincipal, T), ServerRuntimeError> {
    let principal = authorize_request(state, headers, required, is_mutation)?;
    let parsed = serde_json::from_slice::<T>(body)
        .map_err(|err| ServerRuntimeError::BadRequest(format!("invalid JSON body: {err}")))?;
    Ok((principal, parsed))
}

/// Verifies that an already-authenticated principal grants all `required` scopes (`BUG-04`).
pub fn authorize_scopes(
    principal: &AuthenticatedPrincipal,
    required: &[EnvironmentScope],
    is_mutation: bool,
) -> Result<(), ServerRuntimeError> {
    if required.is_empty()
        || principal.method == "local"
        || principal.method == "dev_token"
        || principal.role == "admin"
        || principal.scopes.iter().any(|scope| scope == "*")
    {
        return Ok(());
    }

    let env_scopes: Vec<EnvironmentScope> = principal
        .scopes
        .iter()
        .filter_map(|raw| {
            serde_json::from_value::<EnvironmentScope>(Value::String(raw.clone())).ok()
        })
        .collect();

    if !env_scopes.is_empty() {
        let grant = SessionGrantInput {
            authenticated: true,
            scopes: Some(env_scopes),
            permissions: None,
            server_update_scope: None,
        };
        if grants_all_scopes(&grant, required) {
            return Ok(());
        }
        return Err(forbidden_missing(required));
    }

    let has_client_write = principal.scopes.iter().any(|scope| scope == "client:write");
    if has_client_write {
        return Ok(());
    }

    let has_client_read = principal.scopes.iter().any(|scope| scope == "client:read");
    if !is_mutation && has_client_read {
        return Ok(());
    }

    Err(forbidden_missing(required))
}

fn forbidden_missing(required: &[EnvironmentScope]) -> ServerRuntimeError {
    let names: Vec<&'static str> = required.iter().map(|scope| scope.as_str()).collect();
    ServerRuntimeError::Forbidden(format!("missing required scope(s): {}", names.join(", ")))
}
