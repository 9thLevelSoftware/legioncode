//! Wire envelopes, Legion Connect secret constants, OAuth configs, and errors for
//! [`super::LiveCloudRelayTunnel`] and [`super::CloudConnectManager`].

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::secrets::SecretStoreError;

/// Secret tracking whether Legion Connect revoked this environment link (`cloud/config.ts:20`).
pub const CLOUD_LINK_REVOKED: &str = "cloud-link-revoked";
/// Secret holding the linked Legion account user identifier (`cloud/config.ts:14`).
pub const CLOUD_LINKED_USER_ID: &str = "cloud-linked-user-id";
/// Secret holding the provisioned Legion Connect relay URL (`cloud/config.ts:15`).
pub const RELAY_URL_SECRET: &str = "cloud-relay-url";
/// Secret controlling agent activity publishing (`"true"` / `"false"`, `cloud/config.ts:18`).
pub const PUBLISH_AGENT_ACTIVITY_SECRET: &str = "cloud-publish-agent-activity";
/// Secret indicating desired CLI cloud link mode (`"managed"` or `"publish_only"`, `CliState.ts:18`).
pub const CLOUD_CLI_DESIRED_LINK_SECRET: &str = "cloud-cli-desired-link";
/// Secret storing persisted OAuth CLI token JSON (`CliTokenManager.ts:41`).
pub const CLOUD_CLI_OAUTH_TOKEN_SECRET: &str = "cloud-cli-oauth-token";
/// Secret storing the relay JWT issuer (`cloud/config.ts:16`).
pub const RELAY_ISSUER_SECRET: &str = "cloud-relay-issuer";
/// Secret storing the relay environment credential (`cloud/config.ts:17`).
pub const RELAY_ENVIRONMENT_CREDENTIAL_SECRET: &str = "cloud-relay-environment-credential";
/// Secret storing the cloud mint public key (`cloud/config.ts:11`).
pub const CLOUD_MINT_PUBLIC_KEY: &str = "cloud-mint-ed25519-public-key";
/// Secret storing managed endpoint runtime config (`cloud/config.ts:12`).
pub const CLOUD_ENDPOINT_RUNTIME_CONFIG: &str = "cloud-endpoint-runtime-config";
/// Secret storing confirmed managed endpoint origin (`cloud/config.ts:13`).
pub const CLOUD_ENDPOINT_CONFIRMED_ORIGIN: &str = "cloud-endpoint-confirmed-origin";
/// Secret controlling webhook hold behavior while offline (`cloud/config.ts:19`).
pub const HOLD_WEBHOOKS_WHILE_OFFLINE_SECRET: &str = "cloud-hold-webhooks-while-offline";

/// RFC 8628 device code grant type (`CliTokenManager.ts:44`).
pub const DEVICE_CODE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";

pub(super) const PERSISTED_CLOUD_LINK_SECRETS: &[&str] = &[
    CLOUD_CLI_DESIRED_LINK_SECRET,
    CLOUD_LINKED_USER_ID,
    RELAY_URL_SECRET,
    RELAY_ISSUER_SECRET,
    RELAY_ENVIRONMENT_CREDENTIAL_SECRET,
    CLOUD_MINT_PUBLIC_KEY,
    CLOUD_ENDPOINT_RUNTIME_CONFIG,
    CLOUD_ENDPOINT_CONFIRMED_ORIGIN,
    PUBLISH_AGENT_ACTIVITY_SECRET,
    HOLD_WEBHOOKS_WHILE_OFFLINE_SECRET,
    CLOUD_LINK_REVOKED,
];

/// Incoming HTTP/JSON request envelope delivered over the Legion Connect relay WebSocket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayHttpRequestEnvelope {
    /// Correlation identifier echoed back on every response frame.
    pub request_id: String,
    /// HTTP method (`"GET"`, `"POST"`, `"DELETE"`, etc.).
    pub method: String,
    /// Origin-relative HTTP path and optional query string starting with `'/'`.
    pub path: String,
    /// Request headers forwarded to the local `127.0.0.1` server.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Optional raw UTF-8 request body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Optional structured JSON request body serialized when `body` is omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json: Option<Value>,
}

/// Chunk payload encoding emitted in [`RelayWsOutgoingFrame::ResponseChunk`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayChunkEncoding {
    /// Raw UTF-8 text slice (for JSON/SSE payloads).
    Utf8,
    /// Lowercase hexadecimal encoding for arbitrary binary chunks.
    Hex,
}

/// Outgoing streamed HTTP response frames written back to the Legion Connect relay WebSocket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RelayWsOutgoingFrame {
    /// Initial HTTP status code and response headers.
    #[serde(rename_all = "camelCase")]
    ResponseHead {
        /// Correlated request identifier.
        request_id: String,
        /// HTTP status code returned by the local origin.
        status: u16,
        /// Response headers returned by the local origin.
        headers: BTreeMap<String, String>,
    },
    /// Streamed body chunk produced while draining the local HTTP response.
    #[serde(rename_all = "camelCase")]
    ResponseChunk {
        /// Correlated request identifier.
        request_id: String,
        /// Encoded chunk bytes.
        data: String,
        /// Encoding used for `data` (`"utf8"` or `"hex"`).
        encoding: RelayChunkEncoding,
    },
    /// Terminal end-of-body marker for `request_id`.
    #[serde(rename_all = "camelCase")]
    ResponseEnd {
        /// Correlated request identifier.
        request_id: String,
    },
    /// Error forwarding `request_id` to `http://127.0.0.1:<port>`.
    #[serde(rename_all = "camelCase")]
    ResponseError {
        /// Correlated request identifier.
        request_id: String,
        /// Synthesized HTTP status (`400` invalid envelope or `502` local transport error).
        status: u16,
        /// Diagnostic message.
        message: String,
    },
}

/// CLI desired cloud link exposure mode (`CliState.ts:20`, `connect.ts:216`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloudConnectExposure {
    /// Legion Connect remote exposure is not desired locally.
    Disabled,
    /// Full remote access via managed tunnel.
    Managed,
    /// Publishes agent activity without opening a managed tunnel.
    PublishOnly,
}

/// CLI desired cloud link mode (`CliState.ts:20`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloudCliLinkMode {
    /// Full remote access via managed tunnel.
    Managed,
    /// Publishes agent activity without opening a managed tunnel.
    PublishOnly,
}

/// Provisioning status of the Legion Connect environment link (`connect.ts:194-200`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloudLinkProvisionState {
    /// No `CLOUD_LINKED_USER_ID` secret stored.
    NotProvisioned,
    /// `CLOUD_LINKED_USER_ID` is stored and not revoked.
    Provisioned,
    /// Both `CLOUD_LINKED_USER_ID` and `CLOUD_LINK_REVOKED` are set.
    Revoked,
}

/// Snapshot of Legion Connect state derived from [`crate::secrets::FileSecretStore`] (`connect.ts:146-158`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudConnectStatus {
    /// Local desired exposure setting (`Disabled`, `Managed`, or `PublishOnly`).
    pub exposure: CloudConnectExposure,
    /// Whether an OAuth CLI credential is stored locally.
    pub authenticated: bool,
    /// Provisioning and revocation state of the cloud link.
    pub link_state: CloudLinkProvisionState,
    /// Linked Legion account user ID, if present.
    pub cloud_user_id: Option<String>,
    /// Provisioned relay HTTPS URL, if present.
    pub relay_url: Option<String>,
    /// Whether agent activity publishing is `"true"`.
    pub publish_agent_activity: bool,
}

impl CloudConnectStatus {
    /// Returns `true` when `legioncode connect` link exposure is enabled locally.
    pub const fn is_desired(&self) -> bool {
        !matches!(self.exposure, CloudConnectExposure::Disabled)
    }

    /// Returns `true` when `CLOUD_LINKED_USER_ID` is present.
    pub const fn is_linked(&self) -> bool {
        !matches!(self.link_state, CloudLinkProvisionState::NotProvisioned)
    }

    /// Returns `true` when both `CLOUD_LINKED_USER_ID` and `CLOUD_LINK_REVOKED` exist.
    pub const fn is_revoked(&self) -> bool {
        matches!(self.link_state, CloudLinkProvisionState::Revoked)
    }
}

/// Parameters controlling [`super::CloudConnectManager::disconnect`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CloudDisconnectOptions {
    /// When `true` (`legioncode connect logout`), also deletes `cloud-cli-oauth-token`.
    pub clear_authorization: bool,
    /// Current environment ID used for `DELETE /v1/client/environment-links/<envId>`.
    pub environment_id: Option<String>,
    /// Optional explicit relay base URL overriding `RELAY_URL_SECRET`.
    pub relay_url_override: Option<String>,
}

/// Result of attempting to revoke the environment record on the relay HTTP API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayUnlinkOutcome {
    /// Relay accepted `DELETE` and confirmed `{ ok: true }`.
    Revoked,
    /// Relay confirmed `{ ok: false }` (already unlinked).
    NotLinked,
    /// Skipped remote revoke because OAuth credential, `environment_id`, or `relay_url` was absent.
    NotAuthenticated,
    /// Remote HTTP `DELETE` failed (local link secrets were still cleared).
    Failed(String),
}

/// Summary returned by [`super::CloudConnectManager::disconnect`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudDisconnectReport {
    /// Remote relay revocation result.
    pub relay_outcome: RelayUnlinkOutcome,
    /// Whether local authorization secret was removed.
    pub cleared_authorization: bool,
}

/// Configuration for RFC 8628 OAuth device-code authorization (`publicConfig.ts:166-173`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceAuthorizationConfig {
    /// Device authorization endpoint URL (`/oauth/device_authorization`).
    pub device_authorization_endpoint: String,
    /// Token polling endpoint URL (`/oauth/token`).
    pub token_endpoint: String,
    /// OAuth client identifier.
    pub client_id: String,
    /// Space-delimited or discrete OAuth scopes.
    pub scopes: Vec<String>,
    /// HTTP timeout per request.
    pub request_timeout: Duration,
    /// Increment added when server responds `slow_down` (RFC 8628 default 5s; configurable in tests).
    pub slow_down_increment: Duration,
}

/// Prompt returned to show the user before polling [`super::CloudConnectManager::poll_device_token`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DeviceAuthorizationPrompt {
    /// Opaque device verification code passed to `poll_device_token`.
    pub device_code: String,
    /// Short user code entered in the browser.
    pub user_code: String,
    /// Browser verification URI.
    pub verification_uri: String,
    /// Pre-filled verification URI when supplied by the OAuth provider.
    #[serde(default)]
    pub verification_uri_complete: Option<String>,
    /// Lifetime in seconds before `device_code` expires.
    pub expires_in: u64,
    /// Recommended initial polling interval in seconds.
    #[serde(default)]
    pub interval: Option<u64>,
}

/// Persisted Legion Connect CLI OAuth token record (`CliTokenManager.ts:132-135`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedCloudCliToken {
    /// OAuth bearer access token.
    pub access_token: String,
    /// OAuth refresh token, if issued.
    #[serde(default)]
    pub refresh_token: String,
    /// Expiration timestamp in Unix epoch milliseconds.
    pub expires_at_epoch_ms: i64,
    /// Extracted OIDC user identity (`email` / `preferred_username` / `sub`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
}

/// Errors produced by [`super::LiveCloudRelayTunnel`].
#[derive(Debug, Error)]
pub enum LiveCloudRelayError {
    /// Invalid WebSocket request URL or upgrade header.
    #[error("invalid cloud relay WebSocket URL or request: {detail}")]
    InvalidRequest {
        /// Validation failure detail.
        detail: String,
    },
    /// WebSocket connection or I/O failed.
    #[error("cloud relay WebSocket error: {detail}")]
    WebSocket {
        /// Underlying `tokio_tungstenite` error detail.
        detail: String,
    },
    /// Failed to construct the local HTTP client forwarding requests to `127.0.0.1`.
    #[error("failed to initialize local relay forwarder HTTP client: {detail}")]
    HttpClient {
        /// Underlying error detail.
        detail: String,
    },
    /// Serializing outgoing relay frames failed.
    #[error("failed to serialize cloud relay frame: {source}")]
    Serialize {
        /// Underlying JSON error.
        #[from]
        source: serde_json::Error,
    },
}

/// Errors produced by [`super::CloudConnectManager`].
#[derive(Debug, Error)]
pub enum CloudConnectError {
    /// Secret persistence read/write failed.
    #[error(transparent)]
    SecretStore(#[from] SecretStoreError),
    /// Invalid secret JSON payload.
    #[error("failed to parse stored cloud connect JSON: {source}")]
    Json {
        /// Underlying JSON decode error.
        #[from]
        source: serde_json::Error,
    },
    /// OAuth authorization request or response failed.
    #[error("cloud authorization failed: {detail}")]
    Authorization {
        /// Failure summary.
        detail: String,
    },
    /// Device authorization expired before user approval (`CliTokenManager.ts:247`).
    #[error("Timed out waiting for Legion Connect authorization.")]
    AuthorizationTimeout,
    /// User denied device authorization in the browser (`CliTokenManager.ts:256`).
    #[error("Legion Connect authorization was denied in the browser.")]
    AuthorizationDenied,
}

pub(super) fn current_epoch_ms() -> i64 {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    i64::try_from(ms).unwrap_or(i64::MAX)
}
