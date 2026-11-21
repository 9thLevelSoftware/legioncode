//! Typed host side effects and error definitions for `t3-web-ui`.
//!
//! The pure Rust MVU engine never performs network I/O, timers, or DOM mutations directly;
//! instead [`crate::update::WebAppEngine::dispatch`] returns a deterministic `Vec<WebHostEffect>`
//! for the thin JS/WASM host glue (`app.mjs`) or integration test harness to execute.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use t3_shared::frame_parts::{
    SOCKET_FRAME_PART_BYTES, split_socket_frame_utf16, split_socket_frame_with_limit,
};
use t3_terminal_vt::VtError;
use thiserror::Error;

/// Errors returned by fallible `WebAppEngine` operations (`dispatch_json`, snapshot import, IDs).
#[derive(Debug, Error)]
pub enum WebUiError {
    /// Failed to parse an incoming action JSON payload.
    #[error("invalid WebAppAction JSON: {0}")]
    InvalidActionJson(String),
    /// Failed to parse or validate an incoming `StreamItem` or RPC payload.
    #[error("invalid orchestration stream payload: {0}")]
    InvalidStreamItem(String),
    /// Failed to decode or encode a `"T3VT"` binary terminal snapshot.
    #[error("terminal snapshot error: {0}")]
    InvalidSnapshot(#[from] VtError),
    /// Failed to serialize engine output or snapshot JSON.
    #[error("JSON serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    /// Typed contract identifier validation error.
    #[error("invalid identifier: {0}")]
    InvalidId(String),
    /// Input or action validation error.
    #[error("validation error: {0}")]
    Validation(String),
}

/// Side effect emitted by [`crate::update::WebAppEngine`] for the browser host to execute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum WebHostEffect {
    /// Execute an HTTP `fetch()` against the T3 Code server.
    #[serde(rename_all = "camelCase")]
    HttpFetch {
        /// HTTP method (`"GET"` or `"POST"`).
        method: String,
        /// Relative server path (e.g. `"/api/auth/pair"`, `"/api/projects"`).
        url: String,
        /// Optional UTF-8 JSON request body.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<String>,
        /// Optional bearer session token (`Authorization: Bearer <token>`).
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            alias = "bearer_token"
        )]
        bearer_token: Option<String>,
    },
    /// Open or reconnect the orchestration WebSocket (`?orchestrationProtocol=2&frameParts=1`).
    #[serde(rename_all = "camelCase")]
    WsConnect {
        /// Relative WebSocket URL path + query string.
        url: String,
    },
    /// Send one or more UTF-8-safe WebSocket text frame parts (`t3_shared::frame_parts` `BUG-21`).
    #[serde(rename_all = "camelCase")]
    WsSendFrameParts {
        /// Ordered frame parts ready to transmit sequentially via `ws.send(part)`.
        parts: Vec<String>,
    },
    /// Persist or clear the session bearer token in browser storage (`sessionStorage`).
    #[serde(rename_all = "camelCase")]
    PersistSessionToken {
        /// Bearer session token string (empty clears persisted session).
        token: String,
    },
    /// Move DOM keyboard focus to the first element matching `selector`.
    #[serde(rename_all = "camelCase")]
    FocusSelector {
        /// CSS selector inside the mounted application root.
        selector: String,
    },
}

impl WebHostEffect {
    /// Constructs a `GET` [`WebHostEffect::HttpFetch`].
    pub fn http_get(url: impl Into<String>, bearer_token: Option<String>) -> Self {
        Self::HttpFetch {
            method: "GET".to_owned(),
            url: url.into(),
            body: None,
            bearer_token,
        }
    }

    /// Constructs a `POST` [`WebHostEffect::HttpFetch`] with serialized JSON body.
    pub fn http_post_json(
        url: impl Into<String>,
        body: &Value,
        bearer_token: Option<String>,
    ) -> Result<Self, WebUiError> {
        Ok(Self::HttpFetch {
            method: "POST".to_owned(),
            url: url.into(),
            body: Some(serde_json::to_string(body)?),
            bearer_token,
        })
    }
}

/// Encodes an Effect 4 RPC `Request` envelope and splits it at UTF-8 scalar boundaries using
/// [`t3_shared::frame_parts`] (`BUG-21`).
pub fn encode_rpc_request_frame_parts(
    request_id: u64,
    tag: &str,
    payload: &Value,
    part_bytes: Option<usize>,
) -> Result<Vec<String>, WebUiError> {
    let frame = json!({
        "_tag": "Request",
        "id": request_id.to_string(),
        "tag": tag,
        "payload": payload,
        "headers": [],
    });
    let encoded = serde_json::to_string(&frame)?;
    let limit = part_bytes.unwrap_or(SOCKET_FRAME_PART_BYTES);
    split_socket_frame_with_limit(&encoded, limit)
        .map_err(|err| WebUiError::Validation(err.to_string()))
}

/// Splits raw UTF-16 code units into `?frameParts` WebSocket chunks (`BUG-21`).
pub fn encode_utf16_frame_parts(
    units: &[u16],
    part_bytes: Option<usize>,
) -> Result<Vec<String>, WebUiError> {
    let limit = part_bytes.unwrap_or(SOCKET_FRAME_PART_BYTES);
    split_socket_frame_utf16(units, limit).map_err(|err| WebUiError::Validation(err.to_string()))
}
