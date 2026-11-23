#![forbid(unsafe_code)]
//! Effect RPC wire framing, HTTP/WS auth, MCP boundaries, and provider environment policy.
//!
//! `legion-protocol` provides deterministic, runtime-safe protocol primitives for Wave C
//! and Wave D servers without reading ambient process environment variables or relying on
//! unchecked framing assumptions:
//! - [`rpc_codec`]: Effect 4 `RpcSerialization.layerJson` envelopes, batch decoding, and [`rpc_codec::OutputWindow`] backpressure.
//! - [`auth`]: Header-first [`auth::CredentialPrecedence`] (BUG-03), [`auth::SessionLeaseRegistry`] (BUG-04), and [`auth::TtlSecretCache`] (BUG-15).
//! - [`mcp`]: Pre-auth `/mcp` HTTP gating, `x-legioncode-mode-limit` intersection, `LinkOrigin` fencing, `45`s wait bounds, SSE response matching, and [`mcp::PeerMcpSessionMap`] (BUG-06).
//! - [`provider_env`]: [`provider_env::SanitizedEnv`] (BUG-12), 128 MiB LF-only NDJSON codec, 15-tier maintenance resolution (BUG-13 & BUG-14), and [`provider_env::TerminalSessionKeys`] (BUG-08).
//!
//! # Examples
//!
//! ```
//! use legion_protocol::rpc_codec::{ClientRpcFrame, decode_client_frames};
//!
//! let frames = decode_client_frames(r#"{"_tag":"Ping"}"#)?;
//! assert_eq!(frames, vec![ClientRpcFrame::Ping]);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub mod auth;
pub mod mcp;
pub mod provider_env;
pub mod rpc_codec;
pub mod tunnel;
pub mod url_redaction;
