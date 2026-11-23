//! Integration tests verifying `legion-protocol` wire envelopes, auth/session boundaries,
//! `/mcp` gating & SSE parsing, sanitized environments, NDJSON framing, maintenance precedence,
//! and terminal session eviction keys.

#[path = "protocol_tests/auth.rs"]
mod auth;
#[path = "protocol_tests/mcp.rs"]
mod mcp;
#[path = "protocol_tests/provider_env.rs"]
mod provider_env;
#[path = "protocol_tests/rpc_codec.rs"]
mod rpc_codec;
#[path = "protocol_tests/wave2_deliverables.rs"]
mod wave2_deliverables;
