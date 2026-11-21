//! `/mcp` HTTP boundaries, mode-limit narrowing, `LinkOrigin` fencing, and SSE parsing.
//!
//! Fixes BUG-06 in `apps/server/src/peer/PeerMcpClient.ts` while enforcing the
//! protocol gates from `apps/server/src/mcp/{McpHttpServer.ts,McpInvocationContext.ts,linkOrigin.ts}`.
//!
//! # Examples
//!
//! ```
//! use t3_contracts::orchestration::{LinkOrigin, McpClientAccess, ProviderInteractionMode};
//! use t3_contracts::policy::RuntimeMode;
//! use t3_protocol::mcp::{
//!     MAX_WAIT_TIMEOUT_MS, McpGateOutcome, McpHttpRequest, approved_mode_ceiling,
//!     assert_same_link, clamp_wait_timeout_ms, evaluate_mcp_http_request,
//!     intersect_mode_ceilings, parse_mode_limit, parse_sse_json_rpc_response,
//! };
//! use t3_protocol::rpc_codec::RpcId;
//!
//! // Pre-auth Origin is rejected with 403 even on POST.
//! let req = McpHttpRequest::new("POST", Some("https://evil.example"), None, None);
//! assert_eq!(evaluate_mcp_http_request(&req).err().map(|e| e.status), Some(403));
//!
//! // Valid POST passes when Accept lists both JSON and SSE.
//! let ok_req = McpHttpRequest::new(
//!     "POST",
//!     None,
//!     Some("application/json; charset=utf-8"),
//!     Some("application/json, text/event-stream"),
//! );
//! assert_eq!(evaluate_mcp_http_request(&ok_req)?, McpGateOutcome::JsonRpcPost);
//!
//! // T3-Mode-Limit narrows permissions monotonically.
//! let approved = approved_mode_ceiling(Some(McpClientAccess::FullAccess));
//! let header = parse_mode_limit("auto/plan")?;
//! let effective = intersect_mode_ceilings(approved, Some(header), Some(RuntimeMode::AutoAcceptEdits));
//! assert_eq!(effective.runtime_mode, RuntimeMode::AutoAcceptEdits);
//! assert_eq!(effective.interaction_mode, ProviderInteractionMode::Plan);
//!
//! // Linked callers cannot cross into another link session.
//! let caller = LinkOrigin {
//!     session_id: "link-a".to_owned(),
//!     label: "Peer A".to_owned(),
//!     access: Some(McpClientAccess::Auto),
//! };
//! assert!(assert_same_link(Some(&caller), None).is_err());
//! assert_eq!(clamp_wait_timeout_ms(Some(120_000)), MAX_WAIT_TIMEOUT_MS);
//!
//! // BUG-06: multi-event SSE picks the block matching JSON-RPC id.
//! let sse = "data: {\"jsonrpc\":\"2.0\",\"method\":\"ping\"}\n\n\
//!            data: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"ok\":true}}\n\n";
//! let msg = parse_sse_json_rpc_response(sse, &RpcId::from(7))?;
//! assert_eq!(msg["result"]["ok"], true);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod http_gate;
mod mode_limit;
mod sse;

pub use http_gate::{
    McpGateOutcome, McpGateRejection, McpHttpRequest, evaluate_mcp_http_request,
    parse_mcp_media_types,
};
pub use mode_limit::{
    HEADER_MODE_LIMIT, HEADER_SENDER_THREAD, McpAccessError, ModeCeiling, approved_mode_ceiling,
    assert_not_linked, assert_same_link, format_mode_limit, interaction_mode_rank,
    intersect_mode_ceilings, is_read_only_access, link_origin_is_fenced, parse_mode_limit,
};
pub use sse::{
    DEFAULT_WAIT_TIMEOUT_MS, MAX_WAIT_TIMEOUT_MS, McpSseError, PeerMcpSessionMap,
    PeerMcpSessionState, PeerSessionKey, clamp_wait_timeout_ms, parse_sse_json_rpc_response,
};
