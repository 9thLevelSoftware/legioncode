//! Server-authoritative WebSocket RPC scope and human-client authorization guard (`R5`).

use legion_contracts::permissions::{
    EnvironmentScope, RpcCallerKind, SessionGrantInput, WsMethod, auth_scope_required_response,
    client_only_required_response, requires_human_client_caller, server_required_scopes_for_rpc,
    session_grants_scope,
};
use serde_json::{Value, json};

use super::auth::WsPrincipal;

/// Verifies `principal` caller-kind and exact scope grants before executing a WS RPC.
///
/// Server checks use `permissions: Some(...)` with `server_update_scope` set so servers
/// never expand legacy parent grants.
pub(super) fn authorize_ws_request(
    principal: &WsPrincipal,
    tag: &str,
    payload: &Value,
) -> Result<(), Value> {
    let method = WsMethod::parse(tag).map_err(|_| {
        json!({
            "_tag": "ParseError",
            "message": "WebSocket RPC method name cannot be empty",
        })
    })?;

    if requires_human_client_caller(&method) && principal.caller_kind != RpcCallerKind::Client {
        return Err(client_only_required_response());
    }

    if principal.scopes.iter().any(|s| s == "*") {
        return Ok(());
    }

    let required = server_required_scopes_for_rpc(&method, Some(payload)).map_err(|err| {
        json!({
            "_tag": "ParseError",
            "message": err.to_string(),
        })
    })?;

    let env_scopes: Vec<EnvironmentScope> = principal
        .scopes
        .iter()
        .filter_map(|raw| {
            serde_json::from_value::<EnvironmentScope>(Value::String(raw.clone())).ok()
        })
        .collect();

    let grant = SessionGrantInput {
        authenticated: true,
        scopes: None,
        permissions: Some(env_scopes),
        server_update_scope: Some("environment:maintain".to_owned()),
    };

    if let Some(missing) = required
        .into_iter()
        .find(|scope| !session_grants_scope(&grant, *scope))
    {
        return Err(auth_scope_required_response(missing));
    }

    Ok(())
}
