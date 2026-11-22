//! Shared WebSocket ticket store, scope response splitting, and `ServerAuthDescriptor`.

use std::collections::HashMap;
use std::sync::Mutex as StdMutex;

use legion_persistence::pairing_tokens::ADMINISTRATIVE_CLIENT_SCOPES;
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::http_api::RuntimeHttpState;
use crate::http_auth::AuthenticatedPrincipal;

/// Original 8-scope legacy subset returned in `scopes` (`auth.ts:82-91`).
pub(crate) const LEGACY_SCOPES: &[&str] = &[
    "orchestration:read",
    "orchestration:operate",
    "terminal:operate",
    "review:write",
    "access:read",
    "access:write",
    "relay:read",
    "relay:write",
];

const WS_TICKET_TTL_MS: i64 = 60_000;

#[derive(Debug, Clone)]
struct WsTicketRecord {
    principal: AuthenticatedPrincipal,
    ticket_use: Option<String>,
    expires_at_ms: i64,
}

/// Thread-safe single-use WebSocket upgrade ticket store.
#[derive(Debug, Default)]
pub(crate) struct WsTicketStore {
    tickets: StdMutex<HashMap<String, WsTicketRecord>>,
}

impl WsTicketStore {
    pub(crate) fn issue(
        &self,
        principal: AuthenticatedPrincipal,
        ticket_use: Option<String>,
        now_ms: i64,
    ) -> (String, i64) {
        let ticket = format!("wst_{}", Uuid::new_v4().simple());
        let expires_at_ms = now_ms.saturating_add(WS_TICKET_TTL_MS);
        if let Ok(mut guard) = self.tickets.lock() {
            guard.retain(|_, entry| entry.expires_at_ms > now_ms);
            guard.insert(
                ticket.clone(),
                WsTicketRecord {
                    principal,
                    ticket_use,
                    expires_at_ms,
                },
            );
        }
        (ticket, expires_at_ms)
    }

    pub(crate) fn consume(
        &self,
        ticket: &str,
        expected_use: Option<&str>,
        now_ms: i64,
    ) -> Option<AuthenticatedPrincipal> {
        let mut guard = self.tickets.lock().ok()?;
        guard.retain(|_, entry| entry.expires_at_ms > now_ms);
        let entry = guard.get(ticket)?;
        let use_ok = match (entry.ticket_use.as_deref(), expected_use) {
            (None, _) | (_, None) => true,
            (Some(actual), Some(expected)) => actual == expected,
        };
        if !use_ok || entry.expires_at_ms <= now_ms {
            return None;
        }
        guard.remove(ticket).map(|rec| rec.principal)
    }
}

/// Derives a port- and workspace-scoped cookie name `legioncode_session_<port>_<hash8>`.
pub(crate) fn derive_scoped_session_cookie_name(port: u16, discriminator: &str) -> String {
    let digest = Sha256::digest(discriminator.as_bytes());
    let hex_digest = hex::encode(digest);
    format!("legioncode_session_{port}_{}", &hex_digest[..8])
}

/// Splits full permissions into `(legacy_scopes, all_valid_scopes)`.
pub(crate) fn split_scopes_and_permissions(raw: &[String]) -> (Vec<String>, Vec<String>) {
    let expanded: Vec<String> = if raw.iter().any(|s| s == "*") {
        ADMINISTRATIVE_CLIENT_SCOPES
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    } else {
        raw.to_vec()
    };
    let mut permissions = Vec::new();
    for s in expanded {
        if !permissions.contains(&s) {
            permissions.push(s);
        }
    }
    let legacy = permissions
        .iter()
        .filter(|s| LEGACY_SCOPES.contains(&s.as_str()))
        .cloned()
        .collect();
    (legacy, permissions)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ServerAuthDescriptorPayload<'a> {
    policy: &'a str,
    bootstrap_methods: Vec<&'a str>,
    session_methods: Vec<&'a str>,
    session_cookie_name: &'a str,
    server_update_scope: &'a str,
}

pub(crate) fn build_auth_descriptor(state: &RuntimeHttpState) -> ServerAuthDescriptorPayload<'_> {
    let policy = state.auth_policy.as_str();
    let bootstrap_methods = if policy == "desktop-managed-local" {
        vec!["desktop-bootstrap", "one-time-token"]
    } else {
        vec!["one-time-token"]
    };
    ServerAuthDescriptorPayload {
        policy,
        bootstrap_methods,
        session_methods: vec![
            "browser-session-cookie",
            "bearer-access-token",
            "dpop-access-token",
        ],
        session_cookie_name: &state.cookie_name,
        server_update_scope: "environment:maintain",
    }
}
