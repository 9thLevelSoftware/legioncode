//! Auth-access management snapshot WebSocket stream (`subscribeAuthAccess`).

use legion_orchestrator::time::now_iso_utc;
use legion_persistence::auth_connect_devices::list_connect_devices;
use legion_persistence::auth_records::{
    ClientMetadataRow, list_active_pairing_links, list_active_sessions,
};
use legion_protocol::rpc_codec::RpcExit;
use serde_json::{Value, json};

use super::stream_ctx::StreamContext;
use crate::serve::ws_mcp::rpc_fail;

const STANDARD_AUTH_SCOPES: &[&str] = &[
    "orchestration:read",
    "orchestration:operate",
    "settings:write",
    "providers:manage",
    "environment:maintain",
    "preview:operate",
    "diagnostics:read",
    "terminal:read",
    "terminal:operate",
    "source-control:write",
    "filesystem:read",
    "filesystem:write",
    "access:read",
    "access:write",
    "relay:read",
    "relay:write",
];

const LEGACY_AUTH_SCOPES: &[&str] = &[
    "orchestration:read",
    "orchestration:operate",
    "terminal:operate",
    "review:write",
    "access:read",
    "access:write",
    "relay:read",
    "relay:write",
];

/// Emits the initial `AuthAccessStreamEvent` snapshot and keeps the subscription open.
pub(super) async fn stream_auth_access(ctx: &StreamContext) -> RpcExit {
    let snapshot_res = {
        let now_iso = now_iso_utc();
        let Ok(conn) = ctx.state.runtime_http.db().lock() else {
            return rpc_fail("AuthAccessStreamError", "db mutex poisoned");
        };
        let links = list_active_pairing_links(&conn, &now_iso).map_err(|e| e.to_string());
        let sessions = list_active_sessions(&conn, &now_iso).map_err(|e| e.to_string());
        let devices = list_connect_devices(&conn).map_err(|e| e.to_string());
        match (links, sessions, devices) {
            (Ok(links), Ok(sessions), Ok((_, trusted))) => Ok((links, sessions, trusted)),
            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => Err(e),
        }
    };

    let (links, sessions, trusted_devices) = match snapshot_res {
        Ok(data) => data,
        Err(msg) => return rpc_fail("AuthAccessStreamError", &msg),
    };

    let pairing_links: Vec<Value> = links
        .into_iter()
        .map(|link| {
            let (scopes, permissions) = normalize_scope_lists(&link.scopes);
            let mut obj = json!({
                "id": link.id,
                "scopes": scopes,
                "permissions": permissions,
                "subject": non_empty_str(&link.subject, "cli-user"),
                "createdAt": link.created_at,
                "expiresAt": link.expires_at,
            });
            if let Some(label) = link.label.filter(|s| !s.trim().is_empty())
                && let Some(map) = obj.as_object_mut()
            {
                map.insert("label".to_owned(), Value::String(label));
            }
            obj
        })
        .collect();

    let client_sessions: Vec<Value> = sessions
        .into_iter()
        .map(|sess| {
            let (scopes, permissions) = normalize_scope_lists(&sess.scopes);
            let current = sess.session_id == ctx.principal.session_id;
            json!({
                "sessionId": sess.session_id,
                "subject": non_empty_str(&sess.subject, "client"),
                "scopes": scopes,
                "permissions": permissions,
                "method": map_session_method(&sess.method),
                "client": format_client_meta(&sess.client),
                "issuedAt": sess.issued_at,
                "expiresAt": sess.expires_at,
                "lastConnectedAt": sess.last_connected_at,
                "connected": current || sess.connected,
                "current": current,
            })
        })
        .collect();

    let connect_devices: Vec<Value> = trusted_devices
        .into_iter()
        .map(|dev| {
            let mut obj = json!({
                "proofKeyThumbprint": dev.proof_key_thumbprint,
                "verificationCode": dev.verification_code,
                "approvedAt": dev.approved_at,
            });
            if let Some(client) = dev.client
                && let Some(map) = obj.as_object_mut()
            {
                map.insert("client".to_owned(), format_client_meta(&client));
            }
            obj
        })
        .collect();

    let event = json!({
        "version": 1,
        "revision": 1,
        "type": "snapshot",
        "payload": {
            "pairingLinks": pairing_links,
            "clientSessions": client_sessions,
            "connectDevices": connect_devices,
        },
    });
    if ctx.send_chunk(event).await {
        std::future::pending::<()>().await;
    }
    RpcExit::Success { value: Value::Null }
}

fn normalize_scope_lists(raw: &[String]) -> (Vec<&'static str>, Vec<&'static str>) {
    let mut expanded: Vec<&'static str> = Vec::new();
    for scope in raw {
        if scope == "*" || scope == "owner" || scope == "client:write" {
            for s in STANDARD_AUTH_SCOPES {
                if !expanded.contains(s) {
                    expanded.push(*s);
                }
            }
        } else if scope == "client:read" {
            for s in [
                "orchestration:read",
                "diagnostics:read",
                "terminal:read",
                "filesystem:read",
                "access:read",
                "relay:read",
            ] {
                if !expanded.contains(&s) {
                    expanded.push(s);
                }
            }
        } else if let Some(valid) = STANDARD_AUTH_SCOPES
            .iter()
            .chain(LEGACY_AUTH_SCOPES.iter())
            .copied()
            .find(|known| *known == scope)
            && !expanded.contains(&valid)
        {
            expanded.push(valid);
        }
    }
    let legacy = expanded
        .iter()
        .copied()
        .filter(|s| LEGACY_AUTH_SCOPES.contains(s))
        .collect();
    (legacy, expanded)
}

fn map_session_method(raw: &str) -> &'static str {
    match raw {
        "browser-session-cookie" | "cookie" => "browser-session-cookie",
        "dpop-access-token" => "dpop-access-token",
        _ => "bearer-access-token",
    }
}

fn format_client_meta(row: &ClientMetadataRow) -> Value {
    let device_type = match row.device_type.as_str() {
        "desktop" | "mobile" | "tablet" | "bot" | "unknown" => row.device_type.as_str(),
        _ => "unknown",
    };
    let mut map = serde_json::Map::new();
    map.insert(
        "deviceType".to_owned(),
        Value::String(device_type.to_owned()),
    );
    insert_non_empty(&mut map, "label", row.label.as_deref());
    insert_non_empty(&mut map, "ipAddress", row.ip_address.as_deref());
    insert_non_empty(&mut map, "userAgent", row.user_agent.as_deref());
    insert_non_empty(&mut map, "os", row.os.as_deref());
    insert_non_empty(&mut map, "browser", row.browser.as_deref());
    Value::Object(map)
}

fn insert_non_empty(map: &mut serde_json::Map<String, Value>, key: &str, val: Option<&str>) {
    if let Some(s) = val.map(str::trim).filter(|s| !s.is_empty()) {
        map.insert(key.to_owned(), Value::String(s.to_owned()));
    }
}

fn non_empty_str(s: &str, fallback: &str) -> String {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        fallback.to_owned()
    } else {
        trimmed.to_owned()
    }
}
