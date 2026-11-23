//! Read-only linked-environment thread HTTP/SSE relay (`#39406`), newest-page default
//! (`#39981`, `#40003`), and headless settings hint on link refusal (`#39964`).

use std::time::Duration;

use legion_contracts::environment::EnvironmentCapabilities;
use legion_contracts::peer_links::{
    PEER_LINK_THREAD_HISTORY_ROUTE, PEER_LINK_THREAD_ITEM_ROUTE, PEER_LINK_THREAD_STREAM_ROUTE,
    PeerLinkThreadHistoryInput, PeerLinkThreadInput, PeerLinkThreadItemInput,
    PeerLinkThreadReadError, PeerLinkThreadReadReason,
};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap};
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::Value;

use crate::peer::links::validate_peer_origin;
pub use crate::peer::thread_relay_helpers::{
    NEWEST_PAGE_BEFORE_POSITION, SILENT_STREAM_TIMEOUT_SECS, build_relay_route_url,
    extract_renewed_link_expiry, from_link_error, headless_linked_environments_disabled_hint,
    headless_linked_environments_disabled_hint_for_brand, map_relay_http_status,
    map_relay_http_status_for_brand, newest_page_read_args, parse_sse_stream_items,
    read_newest_or_retry, refuses_before_position,
};

const DEFAULT_RELAY_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Deserialize)]
struct RefusalBody {
    error: String,
}

/// Relay response containing decoded JSON or SSE items plus optional renewed expiry header.
#[derive(Debug, Clone, PartialEq)]
pub struct RelayedThreadPayload<T> {
    /// Decoded payload from the linked peer.
    pub data: T,
    /// Renewed ISO-8601 link expiry from `x-legioncode-link-expires-at`, if sent by the peer.
    pub renewed_expires_at: Option<String>,
}

/// Non-redirecting HTTP/SSE client reading threads, history pages, and turn items from linked
/// environments (`#39406`).
#[derive(Debug, Clone)]
pub struct PeerThreadRelayClient {
    http: Client,
    timeout: Duration,
}

impl PeerThreadRelayClient {
    /// Constructs a [`PeerThreadRelayClient`] with redirects disabled.
    pub fn new() -> Result<Self, PeerLinkThreadReadError> {
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|err| PeerLinkThreadReadError {
                reason: PeerLinkThreadReadReason::Unreachable,
                label: None,
                message: format!("Failed to initialize thread relay client: {err}"),
            })?;
        Ok(Self {
            http,
            timeout: DEFAULT_RELAY_TIMEOUT,
        })
    }

    /// Reads an older history page (`GET /api/links/thread/history`).
    pub async fn history_page(
        &self,
        origin: &str,
        label: &str,
        bearer_token: &str,
        capabilities: &EnvironmentCapabilities,
        input: &PeerLinkThreadHistoryInput,
    ) -> Result<RelayedThreadPayload<Value>, PeerLinkThreadReadError> {
        self.get_json(
            origin,
            label,
            bearer_token,
            capabilities,
            PEER_LINK_THREAD_HISTORY_ROUTE,
            &[
                ("thread", input.thread_id.as_str()),
                ("cursor", input.cursor.as_str()),
            ],
        )
        .await
    }

    /// Reads one full turn item (`GET /api/links/thread/item`).
    pub async fn item(
        &self,
        origin: &str,
        label: &str,
        bearer_token: &str,
        capabilities: &EnvironmentCapabilities,
        input: &PeerLinkThreadItemInput,
    ) -> Result<RelayedThreadPayload<Value>, PeerLinkThreadReadError> {
        let mut params = vec![
            ("thread", input.thread_id.as_str()),
            ("item", input.item_id.as_str()),
        ];
        if let Some(revision) = input.revision.as_deref() {
            params.push(("revision", revision));
        }
        self.get_json(
            origin,
            label,
            bearer_token,
            capabilities,
            PEER_LINK_THREAD_ITEM_ROUTE,
            &params,
        )
        .await
    }

    /// Reads SSE stream events (`GET /api/links/thread/stream`), skipping `unknown-event` rows.
    pub async fn stream_snapshot(
        &self,
        origin: &str,
        label: &str,
        bearer_token: &str,
        capabilities: &EnvironmentCapabilities,
        input: &PeerLinkThreadInput,
    ) -> Result<RelayedThreadPayload<Vec<Value>>, PeerLinkThreadReadError> {
        let (headers, body) = self
            .execute_get(
                origin,
                label,
                bearer_token,
                capabilities,
                PEER_LINK_THREAD_STREAM_ROUTE,
                &[("thread", input.thread_id.as_str())],
            )
            .await?;
        let renewed_expires_at = extract_renewed_link_expiry(&headers);
        let events = parse_sse_stream_items(&body);
        Ok(RelayedThreadPayload {
            data: events,
            renewed_expires_at,
        })
    }

    async fn get_json(
        &self,
        origin: &str,
        label: &str,
        bearer_token: &str,
        capabilities: &EnvironmentCapabilities,
        route: &str,
        params: &[(&str, &str)],
    ) -> Result<RelayedThreadPayload<Value>, PeerLinkThreadReadError> {
        let (headers, body) = self
            .execute_get(origin, label, bearer_token, capabilities, route, params)
            .await?;
        let renewed_expires_at = extract_renewed_link_expiry(&headers);
        let data = serde_json::from_str::<Value>(&body).map_err(|_| PeerLinkThreadReadError {
            reason: PeerLinkThreadReadReason::Unreachable,
            label: Some(label.to_owned()),
            message: format!("{label} returned invalid JSON."),
        })?;
        Ok(RelayedThreadPayload {
            data,
            renewed_expires_at,
        })
    }

    async fn execute_get(
        &self,
        origin: &str,
        label: &str,
        bearer_token: &str,
        capabilities: &EnvironmentCapabilities,
        route: &str,
        params: &[(&str, &str)],
    ) -> Result<(HeaderMap, String), PeerLinkThreadReadError> {
        if capabilities.linked_thread_read != Some(true) {
            return Err(map_relay_http_status(404, None, label));
        }
        let validated_origin =
            validate_peer_origin(origin).map_err(|err| from_link_error(&err, label))?;
        let url = build_relay_route_url(&validated_origin, route, params);
        let resp = self
            .http
            .get(url)
            .header(ACCEPT, "application/json, text/event-stream")
            .header(AUTHORIZATION, format!("Bearer {bearer_token}"))
            .timeout(self.timeout)
            .send()
            .await
            .map_err(|_| PeerLinkThreadReadError {
                reason: PeerLinkThreadReadReason::Unreachable,
                label: Some(label.to_owned()),
                message: format!("{label} did not answer."),
            })?;
        let status = resp.status();
        let headers = resp.headers().clone();
        let body = resp.text().await.unwrap_or_default();
        if status != StatusCode::OK {
            let code = serde_json::from_str::<RefusalBody>(&body)
                .ok()
                .map(|r| r.error);
            return Err(map_relay_http_status(
                status.as_u16(),
                code.as_deref(),
                label,
            ));
        }
        Ok((headers, body))
    }
}
