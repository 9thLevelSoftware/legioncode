//! Live async HTTP peer linking and HTTP/SSE `/mcp` RPC sessions.
//!
//! Pairs [`crate::peer::links::PeerLinksService`] with `/api/auth/pair` and
//! `/.well-known/legioncode/environment` HTTP probes, and wraps `/mcp` JSON-RPC + SSE (`BUG-06` via
//! [`legion_protocol::mcp::parse_sse_json_rpc_response`]), `DELETE /mcp` cleanup, and
//! [`RemoteDelegationTracker`] (`BUG-07`).

#[path = "live_peer_mcp_client.rs"]
mod client;
#[path = "live_peer_mcp_rpc.rs"]
mod rpc;
#[path = "live_peer_mcp_types.rs"]
mod types;

use std::sync::atomic::{AtomicI64, Ordering};

use legion_protocol::mcp::clamp_wait_timeout_ms;
use legion_protocol::rpc_codec::RpcId;
use reqwest::header::CONTENT_TYPE;
use reqwest::{Client, StatusCode};
use rusqlite::Connection;
use serde_json::{Value, json};

use crate::peer::delegation::{
    CallerCancelReason, FollowAction, FollowErrorKind, RemoteDelegationTracker, RemoteStopCommand,
    RemoteTaskTerminalStatus,
};
use crate::peer::links::{
    PeerLinkError, PeerLinkErrorReason, PeerLinksService, validate_peer_origin,
};
use crate::secrets::FileSecretStore;
pub use client::LivePeerClient;
pub use legion_protocol::mcp::parse_sse_json_rpc_response;
use rpc::{
    build_mcp_request_headers, decode_json_or_sse_response, extract_tool_call_result,
    initialize_mcp_session, post_mcp_json, terminate_mcp_session,
};
pub use types::{LivePeerLinkInput, LivePeerMcpOptions, PeerMcpError};

/// Async peer `/mcp` HTTP/SSE session integrating `BUG-06` parsing and `BUG-07` [`RemoteDelegationTracker`].
#[derive(Debug)]
pub struct LivePeerMcpSession {
    http: Client,
    origin: String,
    token: String,
    options: LivePeerMcpOptions,
    session_id: Option<String>,
    protocol_version: Option<String>,
    next_id: AtomicI64,
}

impl LivePeerMcpSession {
    /// Connects and completes MCP `initialize` -> `notifications/initialized` with `DELETE` rollback on error (`BUG-06`).
    pub async fn connect(
        origin: &str,
        token: &str,
        options: LivePeerMcpOptions,
    ) -> Result<Self, PeerMcpError> {
        let normalized_origin = validate_peer_origin(origin)?;
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|err| PeerMcpError::Transport {
                detail: err.to_string(),
            })?;
        let mut session = Self {
            http,
            origin: normalized_origin,
            token: token.to_owned(),
            options,
            session_id: None,
            protocol_version: None,
            next_id: AtomicI64::new(1),
        };
        if let Err(err) = session.initialize().await {
            let _ = session.terminate().await;
            return Err(err);
        }
        Ok(session)
    }

    /// Resolves stored credentials from [`PeerLinksService`] and connects to the peer's `/mcp` endpoint.
    pub async fn connect_peer(
        conn: &Connection,
        secrets: &FileSecretStore,
        environment_id: &str,
        now_iso: &str,
        options: LivePeerMcpOptions,
    ) -> Result<Self, PeerMcpError> {
        let creds = PeerLinksService::new(secrets.clone()).resolve_credentials(
            conn,
            environment_id,
            now_iso,
        )?;
        let probe_client = LivePeerClient::new()?;
        let mut chosen = None;
        for url in &creds.link.urls {
            if let Ok((origin, desc)) = probe_client.probe_environment(url).await
                && desc.environment_id == creds.link.environment_id
            {
                chosen = Some(origin);
                break;
            }
        }
        let target_url = chosen
            .or_else(|| creds.link.urls.into_iter().next())
            .ok_or_else(|| {
                PeerLinkError::new(
                    PeerLinkErrorReason::Unreachable,
                    format!("No peer URL is registered for {}.", creds.link.label),
                )
            })?;
        Self::connect(&target_url, &creds.token, options).await
    }

    /// Returns the validated peer origin URL.
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// Returns the negotiated `mcp-session-id` header, if any.
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Returns the negotiated `mcp-protocol-version` header, if any.
    pub fn protocol_version(&self) -> Option<&str> {
        self.protocol_version.as_deref()
    }

    /// Invokes a peer MCP tool via `POST /mcp`, supporting SSE (`BUG-06`) and `404` session renewal.
    pub async fn call_tool(&mut self, tool: &str, arguments: Value) -> Result<Value, PeerMcpError> {
        let req_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let payload = json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "method": "tools/call",
            "params": { "name": tool, "arguments": arguments }
        });
        let mut resp = self.post_mcp(&payload).await?;
        if resp.status() == StatusCode::NOT_FOUND {
            self.session_id = None;
            self.initialize().await?;
            resp = self.post_mcp(&payload).await?;
        }
        let status = resp.status();
        let content_type = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let body = resp.text().await.map_err(|err| PeerMcpError::Transport {
            detail: err.to_string(),
        })?;
        if !status.is_success() {
            return Err(PeerMcpError::HttpStatus {
                status: status.as_u16(),
                detail: body,
            });
        }
        let entry = decode_json_or_sse_response(&content_type, &body, &RpcId::from(req_id))?;
        extract_tool_call_result(entry)
    }

    /// Sends `legion_thread_interrupt` to the peer `/mcp` server.
    pub async fn interrupt_remote_thread(
        &mut self,
        environment_id: &str,
        thread_id: &str,
        stop: bool,
        reason: Option<&str>,
        idempotency_key: Option<&str>,
    ) -> Result<Value, PeerMcpError> {
        let mut args = json!({
            "environmentId": environment_id,
            "threadId": thread_id,
            "stop": stop,
        });
        if let Some(obj) = args.as_object_mut() {
            if let Some(r) = reason {
                obj.insert("reason".to_owned(), Value::String(r.to_owned()));
            }
            if let Some(key) = idempotency_key {
                obj.insert("clientRequestId".to_owned(), Value::String(key.to_owned()));
            }
        }
        self.call_tool("legion_thread_interrupt", args).await
    }

    /// Executes a [`RemoteStopCommand`] returned by [`RemoteDelegationTracker`] and acknowledges `stop_count`.
    pub async fn execute_stop_command(
        &mut self,
        tracker: &mut RemoteDelegationTracker,
        cmd: &RemoteStopCommand,
    ) -> Result<bool, PeerMcpError> {
        let RemoteStopCommand::StopThere {
            environment_id,
            thread_id,
            idempotency_key,
            stop_count,
            reason,
        } = cmd
        else {
            return Ok(false);
        };
        let _ = self
            .interrupt_remote_thread(
                environment_id.as_str(),
                thread_id.as_str(),
                true,
                reason.as_deref(),
                Some(idempotency_key.as_str()),
            )
            .await?;
        let _ = tracker.accept_stop_acknowledgement(*stop_count);
        Ok(true)
    }

    /// Cancels a delegated task via `tracker.on_caller_cancel(reason)` and stops the remote child over `/mcp`.
    pub async fn cancel_delegated_task(
        &mut self,
        tracker: &mut RemoteDelegationTracker,
        reason: CallerCancelReason,
    ) -> Result<bool, PeerMcpError> {
        let cmd = tracker.on_caller_cancel(reason);
        self.execute_stop_command(tracker, &cmd).await
    }

    /// Applies `error` to `tracker` and executes `legion_thread_interrupt` whenever `SendStop(StopThere)` emits (`BUG-07`).
    pub async fn handle_follow_error(
        &mut self,
        tracker: &mut RemoteDelegationTracker,
        error: FollowErrorKind,
        message: &str,
    ) -> Result<FollowAction, PeerMcpError> {
        let action = tracker.on_follow_error(error, message);
        if let FollowAction::SendStop(ref cmd) = action {
            let _ = self.execute_stop_command(tracker, cmd).await?;
        }
        Ok(action)
    }

    /// Executes one bounded `legion_thread_wait` slice for `tracker`, preserving `BUG-07` stop-on-delete semantics.
    pub async fn wait_delegated_slice(
        &mut self,
        tracker: &mut RemoteDelegationTracker,
    ) -> Result<FollowAction, PeerMcpError> {
        let requested = tracker.next_wait_slice_ms().unwrap_or(15_000);
        let timeout_ms = clamp_wait_timeout_ms(i64::try_from(requested).ok());
        let child = tracker.remote_child();
        let mut args = json!({
            "environmentId": child.environment_id.as_str(),
            "threadId": child.thread_id.as_str(),
            "timeoutMs": timeout_ms,
            "untilDelegatedWorkSettled": true,
        });
        if let Some(run_id) = &child.run_id
            && let Some(obj) = args.as_object_mut()
        {
            obj.insert(
                "runId".to_owned(),
                Value::String(run_id.as_str().to_owned()),
            );
        }

        match self.call_tool("legion_thread_wait", args).await {
            Ok(result) => {
                if result.get("timedOut").and_then(Value::as_bool) == Some(true) {
                    return Ok(tracker.record_wait_slice_timeout(timeout_ms));
                }
                let terminal = match result.get("status").and_then(Value::as_str) {
                    Some("failed") => RemoteTaskTerminalStatus::Failed,
                    Some("cancelled") => RemoteTaskTerminalStatus::Cancelled,
                    Some("interrupted") => RemoteTaskTerminalStatus::Interrupted,
                    _ => RemoteTaskTerminalStatus::Completed,
                };
                Ok(FollowAction::CompleteLocal {
                    status: terminal,
                    result: serde_json::to_string(&result).unwrap_or_default(),
                    stopped_there: None,
                    stop_count: tracker.remote_child().stop_count,
                    executed_model: None,
                })
            }
            Err(PeerMcpError::ToolRefused { code, message }) => {
                let kind = match code.as_str() {
                    "thread_not_found" => FollowErrorKind::ThreadNotFound,
                    "run_not_found" => FollowErrorKind::RunNotFound,
                    "capability_denied" => FollowErrorKind::CapabilityDenied,
                    "invalid_request" => FollowErrorKind::InvalidRequest { link_exists: true },
                    _ => FollowErrorKind::Transient,
                };
                self.handle_follow_error(tracker, kind, &message).await
            }
            Err(err) => Err(err),
        }
    }

    /// Sends `DELETE /mcp` for the current session and consumes `self` (`BUG-06`).
    pub async fn close(mut self) -> Result<(), PeerMcpError> {
        self.terminate().await
    }

    /// Sends `DELETE /mcp` if a session is active, clearing stored session state (`BUG-06`).
    pub async fn terminate(&mut self) -> Result<(), PeerMcpError> {
        let Some(sid) = self.session_id.take() else {
            return Ok(());
        };
        terminate_mcp_session(&self.http, &self.origin, &self.token, &sid, &self.options).await
    }

    async fn initialize(&mut self) -> Result<(), PeerMcpError> {
        let init_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sid, proto) = initialize_mcp_session(
            &self.http,
            &self.origin,
            &self.token,
            &self.options,
            init_id,
        )
        .await?;
        self.session_id = Some(sid);
        self.protocol_version = Some(proto);
        Ok(())
    }

    async fn post_mcp(&self, payload: &Value) -> Result<reqwest::Response, PeerMcpError> {
        let headers = build_mcp_request_headers(
            &self.token,
            self.session_id.as_deref(),
            self.protocol_version.as_deref(),
            &self.options,
        )?;
        post_mcp_json(
            &self.http,
            &self.origin,
            headers,
            payload,
            self.options.timeout,
        )
        .await
    }
}
