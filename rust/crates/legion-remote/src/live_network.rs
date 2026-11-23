//! Live async networking for SSH tunnels, Tailscale CLI & Serve, peer `/mcp` RPC, and Legion Connect.
//!
//! Pairs the pure protocol/state crates ([`crate::tunnel`], [`crate::peer`], [`crate::secrets`])
//! with real `tokio::process`, `reqwest`, and `tokio_tungstenite` execution.

#[path = "live_cloud_relay.rs"]
pub mod live_cloud_relay;
#[path = "live_peer_mcp.rs"]
pub mod live_peer_mcp;
#[path = "live_ssh_tailscale.rs"]
pub mod live_ssh_tailscale;

pub use live_cloud_relay::{
    CLOUD_CLI_DESIRED_LINK_SECRET, CLOUD_CLI_OAUTH_TOKEN_SECRET, CLOUD_ENDPOINT_CONFIRMED_ORIGIN,
    CLOUD_ENDPOINT_RUNTIME_CONFIG, CLOUD_LINK_REVOKED, CLOUD_LINKED_USER_ID, CLOUD_MINT_PUBLIC_KEY,
    CloudCliLinkMode, CloudConnectError, CloudConnectExposure, CloudConnectManager,
    CloudConnectStatus, CloudDisconnectOptions, CloudDisconnectReport, CloudLinkProvisionState,
    DEVICE_CODE_GRANT_TYPE, DeviceAuthorizationConfig, DeviceAuthorizationPrompt,
    HOLD_WEBHOOKS_WHILE_OFFLINE_SECRET, LiveCloudRelayError, LiveCloudRelayTunnel,
    PUBLISH_AGENT_ACTIVITY_SECRET, PersistedCloudCliToken, RELAY_ENVIRONMENT_CREDENTIAL_SECRET,
    RELAY_ISSUER_SECRET, RELAY_URL_SECRET, RelayChunkEncoding, RelayHttpRequestEnvelope,
    RelayUnlinkOutcome, RelayWsOutgoingFrame,
};
pub use live_peer_mcp::{
    LivePeerClient, LivePeerLinkInput, LivePeerMcpOptions, LivePeerMcpSession,
    OAUTH_MCP_REVOKE_ROUTE, PeerMcpError, parse_sse_json_rpc_response,
};
pub use live_ssh_tailscale::{
    LiveSshError, LiveSshForwardHandle, LiveSshTunnelManager, LiveTailscaleController,
};
