//! MCP HTTP/SSE client, `acp-mcp-call` helper, and `acp-mcp-bridge` stdio-to-HTTP relay.
//!
//! Mirrors `apps/server/src/mcp/AcpMcpStdioBridge.ts` and fixes `BUG-06` (SSE multi-event parsing,
//! server `ping` auto-replies, and `DELETE` session termination on close or failed handshake).

#[path = "acp_mcp_sse.rs"]
mod sse;
#[path = "acp_mcp_stdio.rs"]
mod stdio;

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use reqwest::Client;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde_json::{Value, json};
use t3_contracts::brand::{ACTIVE_BRAND, BrandConfig};
use thiserror::Error;
use tokio::sync::Mutex;

pub use sse::strip_duplicate_structured_text;
use sse::{
    decode_response_entries, is_server_request, protocol_version_of, reply_to_server_request,
};
pub use stdio::{call_mcp_tool, run_mcp_stdio_bridge};

/// Negotiated MCP protocol version sent on `initialize` (`AcpMcpStdioBridge.ts:38`).
pub const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

const HEADER_MCP_SESSION_ID: &str = "mcp-session-id";
const HEADER_MCP_PROTOCOL_VERSION: &str = "mcp-protocol-version";

/// Returns candidate MCP bridge endpoint environment variable names for [`ACTIVE_BRAND`].
#[must_use]
pub fn mcp_bridge_url_env_candidates() -> Vec<String> {
    ACTIVE_BRAND.env_var_candidates("ACP_MCP_URL")
}

/// Resolves the MCP bridge endpoint URL from environment lookup using `brand`.
#[must_use]
pub fn resolve_mcp_bridge_url_for_brand<F>(brand: &BrandConfig, lookup: F) -> Option<String>
where
    F: FnMut(&str) -> Option<String>,
{
    brand.read_env_from("ACP_MCP_URL", lookup)
}

/// Errors returned by [`McpHttpClient`], [`call_mcp_tool`], and [`run_mcp_stdio_bridge`].
#[derive(Debug, Error)]
pub enum McpBridgeError {
    /// Target `/mcp` endpoint returned a non-2xx HTTP status code.
    #[error("{} MCP endpoint responded with HTTP {status}.", ACTIVE_BRAND.product_name)]
    HttpStatus {
        /// Returned HTTP status code.
        status: u16,
    },
    /// Underlying HTTP transport or streaming read error.
    #[error("MCP HTTP transport failed: {detail}")]
    Transport {
        /// Human-readable error detail.
        detail: String,
    },
    /// JSON-RPC request returned a JSON-RPC `error` object or omitted its matching response.
    #[error("{} MCP {method} failed{detail}", ACTIVE_BRAND.product_name)]
    JsonRpc {
        /// JSON-RPC method invoked (e.g. `"initialize"`, `"tools/call"`).
        method: String,
        /// Formatted failure suffix (`"."` or `": <json>"`).
        detail: String,
    },
    /// Malformed JSON payload received from CLI arguments or server body.
    #[error("{detail}")]
    InvalidInput {
        /// Validation error message matching `AcpMcpStdioBridge.ts`.
        detail: String,
    },
    /// Malformed SSE or JSON response body from the MCP server.
    #[error("failed to parse MCP response: {detail}")]
    Parse {
        /// Parse failure reason.
        detail: String,
    },
    /// Stdin/stdout I/O error inside [`run_mcp_stdio_bridge`].
    #[error("MCP stdio bridge I/O failed: {source}")]
    Io {
        /// Underlying I/O error.
        #[from]
        source: std::io::Error,
    },
}

#[derive(Debug, Default)]
struct SessionHeaders {
    session_id: Option<String>,
    protocol_version: Option<String>,
}

/// Stateful initialized MCP HTTP/SSE session closing via `DELETE` on [`Self::close`].
#[derive(Debug, Clone)]
pub struct McpHttpClient {
    http: Client,
    endpoint: String,
    authorization: String,
    headers: Arc<Mutex<SessionHeaders>>,
    next_id: Arc<AtomicU64>,
}

impl McpHttpClient {
    /// Creates an uninitialized client handle (`run_mcp_stdio_bridge` forwards caller `initialize`).
    pub fn new_uninitialized(
        endpoint: impl Into<String>,
        authorization: impl Into<String>,
    ) -> Result<Self, McpBridgeError> {
        let http = Client::builder()
            .build()
            .map_err(|err| McpBridgeError::Transport {
                detail: err.to_string(),
            })?;
        Ok(Self {
            http,
            endpoint: endpoint.into(),
            authorization: authorization.into(),
            headers: Arc::new(Mutex::new(SessionHeaders::default())),
            next_id: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Initializes an MCP session (`initialize` -> `notifications/initialized`) with rollback `DELETE` on error (`BUG-06`).
    pub async fn connect(
        endpoint: impl Into<String>,
        authorization: impl Into<String>,
        client_name: &str,
    ) -> Result<Self, McpBridgeError> {
        let client = Self::new_uninitialized(endpoint, authorization)?;
        if let Err(err) = client.initialize(client_name).await {
            client.terminate_session().await;
            return Err(err);
        }
        Ok(client)
    }

    /// Returns the active `mcp-session-id` header if assigned by the server.
    pub async fn session_id(&self) -> Option<String> {
        self.headers.lock().await.session_id.clone()
    }

    /// Returns the active negotiated `mcp-protocol-version`, if observed.
    pub async fn protocol_version(&self) -> Option<String> {
        self.headers.lock().await.protocol_version.clone()
    }

    /// Returns the configured `/mcp` endpoint URL.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    async fn initialize(&self, client_name: &str) -> Result<(), McpBridgeError> {
        let _ = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": { "name": client_name, "version": "0.0.0" }
                }),
            )
            .await?;
        let _ = self
            .send_internal(&json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            }))
            .await?;
        Ok(())
    }

    /// Invokes an MCP tool (`tools/call`) and returns its `result` payload.
    pub async fn call_tool(&self, tool: &str, args: &Value) -> Result<Value, McpBridgeError> {
        let envelope = self
            .request("tools/call", json!({ "name": tool, "arguments": args }))
            .await?;
        Ok(envelope.get("result").cloned().unwrap_or(Value::Null))
    }

    /// Sends a typed JSON-RPC request, auto-replying to server `ping`s (`AcpMcpStdioBridge.ts:125-146`).
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, McpBridgeError> {
        let seq = self.next_id.fetch_add(1, Ordering::Relaxed);
        let id = format!("{}-cli-{seq}", ACTIVE_BRAND.cli_bin_name);
        let body = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        let payloads = self.send_internal(&body).await?;
        let found = payloads
            .into_iter()
            .find(|entry| entry.get("id").and_then(Value::as_str) == Some(id.as_str()));
        let Some(envelope) = found else {
            return Err(McpBridgeError::JsonRpc {
                method: method.to_owned(),
                detail: ".".to_owned(),
            });
        };
        if let Some(error) = envelope.get("error").filter(|e| !e.is_null()) {
            return Err(McpBridgeError::JsonRpc {
                method: method.to_owned(),
                detail: format!(": {error}"),
            });
        }
        Ok(envelope)
    }

    /// Forwards a raw JSON-RPC payload verbatim without consuming server requests, returning response messages.
    pub async fn forward_raw(&self, message: &Value) -> Result<Vec<Value>, McpBridgeError> {
        self.post_json_collect(message, true).await
    }

    /// Sends `DELETE` when a session id was negotiated (`BUG-06`).
    pub async fn close(self) {
        self.terminate_session().await;
    }

    /// Best-effort `DELETE` request ending the active MCP HTTP session.
    pub async fn terminate_session(&self) {
        let Ok(Some(header_map)) = self.build_headers(false).await else {
            return;
        };
        let _ = self
            .http
            .delete(&self.endpoint)
            .headers(header_map)
            .send()
            .await;
    }

    async fn send_internal(&self, message: &Value) -> Result<Vec<Value>, McpBridgeError> {
        let mut queue: VecDeque<Value> = VecDeque::new();
        queue.push_back(message.clone());
        let mut out = Vec::new();
        while let Some(outgoing) = queue.pop_front() {
            let received = self.post_json_collect(&outgoing, true).await?;
            for entry in received {
                if is_server_request(&entry) {
                    queue.push_back(reply_to_server_request(&entry));
                } else {
                    out.push(entry);
                }
            }
        }
        Ok(out)
    }

    async fn post_json_collect(
        &self,
        message: &Value,
        track_protocol: bool,
    ) -> Result<Vec<Value>, McpBridgeError> {
        let headers = self
            .build_headers(true)
            .await?
            .unwrap_or_else(HeaderMap::new);
        let resp = self
            .http
            .post(&self.endpoint)
            .headers(headers)
            .json(message)
            .send()
            .await
            .map_err(|err| McpBridgeError::Transport {
                detail: err.to_string(),
            })?;

        if let Some(sid) = resp
            .headers()
            .get(HEADER_MCP_SESSION_ID)
            .and_then(|v| v.to_str().ok())
            .filter(|s| !s.is_empty())
        {
            self.headers.lock().await.session_id = Some(sid.to_owned());
        }

        let status = resp.status();
        if !status.is_success() {
            let _ = resp.bytes().await;
            return Err(McpBridgeError::HttpStatus {
                status: status.as_u16(),
            });
        }

        let entries = decode_response_entries(resp).await?;
        let mut collected = Vec::with_capacity(entries.len());
        for entry in entries {
            if let Some(arr) = entry.as_array() {
                for item in arr {
                    if track_protocol && let Some(ver) = protocol_version_of(item) {
                        self.headers.lock().await.protocol_version = Some(ver.to_owned());
                    }
                    collected.push(item.clone());
                }
            } else {
                if track_protocol && let Some(ver) = protocol_version_of(&entry) {
                    self.headers.lock().await.protocol_version = Some(ver.to_owned());
                }
                collected.push(entry);
            }
        }
        Ok(collected)
    }

    async fn build_headers(
        &self,
        include_content_type: bool,
    ) -> Result<Option<HeaderMap>, McpBridgeError> {
        let snapshot = self.headers.lock().await;
        if !include_content_type && snapshot.session_id.is_none() {
            return Ok(None);
        }
        let mut map = HeaderMap::new();
        map.insert(
            ACCEPT,
            HeaderValue::from_static("application/json, text/event-stream"),
        );
        if include_content_type {
            map.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        }
        let auth = HeaderValue::from_str(&self.authorization).map_err(|err| {
            McpBridgeError::InvalidInput {
                detail: format!("invalid authorization header: {err}"),
            }
        })?;
        map.insert(AUTHORIZATION, auth);
        if let Some(sid) = &snapshot.session_id
            && let Ok(val) = HeaderValue::from_str(sid)
        {
            map.insert(HeaderName::from_static(HEADER_MCP_SESSION_ID), val);
        }
        if let Some(ver) = &snapshot.protocol_version
            && let Ok(val) = HeaderValue::from_str(ver)
        {
            map.insert(HeaderName::from_static(HEADER_MCP_PROTOCOL_VERSION), val);
        }
        Ok(Some(map))
    }
}
