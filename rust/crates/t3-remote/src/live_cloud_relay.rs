//! Live T3 Connect WebSocket relay tunnel client and CLI secret/OAuth/disconnect manager.
//!
//! Mirrors `apps/server/src/cli/connect.ts`, `apps/server/src/cloud/config.ts`, and
//! `apps/server/src/cloud/CliTokenManager.ts`.

#[path = "live_cloud_relay_http.rs"]
mod http_ops;
#[path = "live_cloud_relay_oauth.rs"]
mod oauth_ops;
#[path = "live_cloud_relay_types.rs"]
mod types;

use std::time::Duration;

use reqwest::Client;
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::{
    AUTHORIZATION as WS_AUTHORIZATION, HeaderValue as WsHeaderValue,
};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

use crate::secrets::FileSecretStore;
use http_ops::run_relay_ws_loop;
use oauth_ops::{
    poll_device_authorization_token, request_device_authorization, revoke_remote_environment_link,
};
use types::PERSISTED_CLOUD_LINK_SECRETS;
pub use types::{
    CLOUD_CLI_DESIRED_LINK_SECRET, CLOUD_CLI_OAUTH_TOKEN_SECRET, CLOUD_ENDPOINT_CONFIRMED_ORIGIN,
    CLOUD_ENDPOINT_RUNTIME_CONFIG, CLOUD_LINK_REVOKED, CLOUD_LINKED_USER_ID, CLOUD_MINT_PUBLIC_KEY,
    CloudCliLinkMode, CloudConnectError, CloudConnectExposure, CloudConnectStatus,
    CloudDisconnectOptions, CloudDisconnectReport, CloudLinkProvisionState, DEVICE_CODE_GRANT_TYPE,
    DeviceAuthorizationConfig, DeviceAuthorizationPrompt, HOLD_WEBHOOKS_WHILE_OFFLINE_SECRET,
    LiveCloudRelayError, PUBLISH_AGENT_ACTIVITY_SECRET, PersistedCloudCliToken,
    RELAY_ENVIRONMENT_CREDENTIAL_SECRET, RELAY_ISSUER_SECRET, RELAY_URL_SECRET, RelayChunkEncoding,
    RelayHttpRequestEnvelope, RelayUnlinkOutcome, RelayWsOutgoingFrame,
};

/// Active WebSocket client forwarding relay HTTP envelopes to `http://127.0.0.1:<local_port>`.
#[derive(Debug)]
pub struct LiveCloudRelayTunnel {
    ws_url: String,
    local_port: u16,
    http: Client,
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

impl LiveCloudRelayTunnel {
    /// Connects to the WebSocket relay URL without a bearer token.
    pub async fn connect(
        ws_url: impl Into<String>,
        local_port: u16,
    ) -> Result<Self, LiveCloudRelayError> {
        Self::connect_with_bearer(ws_url, local_port, None).await
    }

    /// Connects to `ws_url` (adding `Authorization: Bearer <token>` when supplied) for `local_port`.
    pub async fn connect_with_bearer(
        ws_url: impl Into<String>,
        local_port: u16,
        bearer_token: Option<&str>,
    ) -> Result<Self, LiveCloudRelayError> {
        let ws_url = ws_url.into();
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|err| LiveCloudRelayError::HttpClient {
                detail: err.to_string(),
            })?;
        let mut request = ws_url.as_str().into_client_request().map_err(|err| {
            LiveCloudRelayError::InvalidRequest {
                detail: err.to_string(),
            }
        })?;
        if let Some(tok) = bearer_token {
            let val = WsHeaderValue::from_str(&format!("Bearer {tok}")).map_err(|err| {
                LiveCloudRelayError::InvalidRequest {
                    detail: format!("invalid relay bearer token header: {err}"),
                }
            })?;
            request.headers_mut().insert(WS_AUTHORIZATION, val);
        }
        let (ws, _) =
            connect_async(request)
                .await
                .map_err(|err| LiveCloudRelayError::WebSocket {
                    detail: err.to_string(),
                })?;
        Ok(Self {
            ws_url,
            local_port,
            http,
            ws,
        })
    }

    /// Returns the WebSocket relay URL.
    pub fn ws_url(&self) -> &str {
        &self.ws_url
    }

    /// Returns the forwarded local `127.0.0.1` port.
    pub fn local_port(&self) -> u16 {
        self.local_port
    }

    /// Runs the relay loop forwarding incoming HTTP envelopes until WS close or `shutdown` triggers.
    pub async fn run_until_shutdown(
        self,
        shutdown: watch::Receiver<bool>,
    ) -> Result<(), LiveCloudRelayError> {
        run_relay_ws_loop(self.ws, self.http, self.local_port, shutdown).await
    }
}

/// Manages T3 Connect secrets in [`FileSecretStore`], RFC 8628 device-code polling, and disconnect.
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use t3_remote::live_network::{
///     CloudCliLinkMode, CloudConnectExposure, CloudConnectManager, CloudLinkProvisionState,
/// };
/// use t3_remote::secrets::FileSecretStore;
///
/// let dir = tempfile::tempdir()?;
/// let mgr = CloudConnectManager::new(FileSecretStore::from_state_dir(dir.path())?)?;
/// mgr.set_desired_link(true, CloudCliLinkMode::Managed)?;
/// mgr.set_linked_environment("user-1", "https://relay.example.test")?;
/// mgr.mark_link_revoked("revoked")?;
/// let status = mgr.read_status()?;
/// assert_eq!(status.exposure, CloudConnectExposure::Managed);
/// assert_eq!(status.link_state, CloudLinkProvisionState::Revoked);
/// assert!(status.is_revoked());
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct CloudConnectManager {
    secrets: FileSecretStore,
    http: Client,
}

impl CloudConnectManager {
    /// Creates a [`CloudConnectManager`] backed by `secrets` and a non-redirecting HTTP client.
    pub fn new(secrets: FileSecretStore) -> Result<Self, CloudConnectError> {
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|err| CloudConnectError::Authorization {
                detail: format!("failed to build cloud HTTP client: {err}"),
            })?;
        Ok(Self { secrets, http })
    }

    /// Returns a reference to the backing [`FileSecretStore`].
    pub fn secrets(&self) -> &FileSecretStore {
        &self.secrets
    }

    /// Reads current T3 Connect state from [`FileSecretStore`] (`connect.ts:675-712`).
    pub fn read_status(&self) -> Result<CloudConnectStatus, CloudConnectError> {
        let desired_raw = self.secrets.get_utf8(CLOUD_CLI_DESIRED_LINK_SECRET)?;
        let exposure = match desired_raw.as_deref() {
            None => CloudConnectExposure::Disabled,
            Some("publish_only") => CloudConnectExposure::PublishOnly,
            Some(_) => CloudConnectExposure::Managed,
        };
        let cloud_user_id = self.secrets.get_utf8(CLOUD_LINKED_USER_ID)?;
        let relay_url = self.secrets.get_utf8(RELAY_URL_SECRET)?;
        let publish_raw = self.secrets.get_utf8(PUBLISH_AGENT_ACTIVITY_SECRET)?;
        let revoked_mark = self.secrets.get(CLOUD_LINK_REVOKED)?.is_some();
        let authenticated = self.load_oauth_token()?.is_some();
        let link_state = if cloud_user_id.is_none() {
            CloudLinkProvisionState::NotProvisioned
        } else if revoked_mark {
            CloudLinkProvisionState::Revoked
        } else {
            CloudLinkProvisionState::Provisioned
        };

        Ok(CloudConnectStatus {
            exposure,
            authenticated,
            link_state,
            cloud_user_id,
            relay_url,
            publish_agent_activity: publish_raw.as_deref() == Some("true"),
        })
    }

    /// Sets or clears `CLOUD_CLI_DESIRED_LINK_SECRET` (`CliState.ts:41-54`).
    pub fn set_desired_link(
        &self,
        desired: bool,
        mode: CloudCliLinkMode,
    ) -> Result<(), CloudConnectError> {
        if desired {
            let val = match mode {
                CloudCliLinkMode::Managed => "managed",
                CloudCliLinkMode::PublishOnly => "publish_only",
            };
            self.secrets.set_utf8(CLOUD_CLI_DESIRED_LINK_SECRET, val)?;
        } else {
            let _ = self.secrets.remove(CLOUD_CLI_DESIRED_LINK_SECRET)?;
        }
        Ok(())
    }

    /// Updates `PUBLISH_AGENT_ACTIVITY_SECRET` and cancels pending `publish_only` desires when disabling.
    pub fn set_publish_agent_activity(&self, enabled: bool) -> Result<(), CloudConnectError> {
        self.secrets.set_utf8(
            PUBLISH_AGENT_ACTIVITY_SECRET,
            if enabled { "true" } else { "false" },
        )?;
        if !enabled {
            let status = self.read_status()?;
            if !status.is_linked() && status.exposure == CloudConnectExposure::PublishOnly {
                self.set_desired_link(false, CloudCliLinkMode::Managed)?;
            }
        }
        Ok(())
    }

    /// Stores `CLOUD_LINKED_USER_ID` and `RELAY_URL_SECRET` for an active cloud environment link.
    pub fn set_linked_environment(
        &self,
        cloud_user_id: &str,
        relay_url: &str,
    ) -> Result<(), CloudConnectError> {
        self.secrets.set_utf8(CLOUD_LINKED_USER_ID, cloud_user_id)?;
        self.secrets.set_utf8(RELAY_URL_SECRET, relay_url)?;
        Ok(())
    }

    /// Marks the active cloud link as revoked (`CLOUD_LINK_REVOKED`).
    pub fn mark_link_revoked(&self, reason: &str) -> Result<(), CloudConnectError> {
        self.secrets.set_utf8(CLOUD_LINK_REVOKED, reason)?;
        Ok(())
    }

    /// Drops a revoked cloud link cleanly (`CliState.ts:56-74`), returning whether one existed.
    pub fn drop_revoked_link(&self) -> Result<bool, CloudConnectError> {
        if self.secrets.get(CLOUD_LINK_REVOKED)?.is_none() {
            return Ok(false);
        }
        for key in [
            CLOUD_LINKED_USER_ID,
            RELAY_ENVIRONMENT_CREDENTIAL_SECRET,
            CLOUD_MINT_PUBLIC_KEY,
            CLOUD_ENDPOINT_RUNTIME_CONFIG,
            CLOUD_ENDPOINT_CONFIRMED_ORIGIN,
        ] {
            let _ = self.secrets.remove(key)?;
        }
        let _ = self.secrets.remove(CLOUD_LINK_REVOKED)?;
        Ok(true)
    }

    /// Persists the CLI OAuth token JSON into `CLOUD_CLI_OAUTH_TOKEN_SECRET`.
    pub fn store_oauth_token(
        &self,
        token: &PersistedCloudCliToken,
    ) -> Result<(), CloudConnectError> {
        let encoded = serde_json::to_string(token)?;
        self.secrets
            .set_utf8(CLOUD_CLI_OAUTH_TOKEN_SECRET, &encoded)?;
        Ok(())
    }

    /// Loads the persisted CLI OAuth token JSON from `CLOUD_CLI_OAUTH_TOKEN_SECRET`.
    pub fn load_oauth_token(&self) -> Result<Option<PersistedCloudCliToken>, CloudConnectError> {
        let Some(raw) = self.secrets.get_utf8(CLOUD_CLI_OAUTH_TOKEN_SECRET)? else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_str(&raw)?))
    }

    /// Removes `CLOUD_CLI_OAUTH_TOKEN_SECRET` if present.
    pub fn clear_oauth_token(&self) -> Result<bool, CloudConnectError> {
        Ok(self.secrets.remove(CLOUD_CLI_OAUTH_TOKEN_SECRET)?)
    }

    /// Starts an RFC 8628 device authorization flow.
    pub async fn start_device_authorization(
        &self,
        cfg: &DeviceAuthorizationConfig,
    ) -> Result<DeviceAuthorizationPrompt, CloudConnectError> {
        request_device_authorization(&self.http, cfg).await
    }

    /// Polls the token endpoint for `device_code` and persists the resulting token (`CliTokenManager.ts:394`).
    pub async fn poll_device_token(
        &self,
        cfg: &DeviceAuthorizationConfig,
        device_code: &str,
        initial_interval: Duration,
        expires_in: Duration,
    ) -> Result<PersistedCloudCliToken, CloudConnectError> {
        let token = poll_device_authorization_token(
            &self.http,
            cfg,
            device_code,
            initial_interval,
            expires_in,
        )
        .await?;
        self.store_oauth_token(&token)?;
        Ok(token)
    }

    /// Disconnects T3 Connect (`connect.ts:485-510`), clearing secrets and optionally logging out.
    pub async fn disconnect(
        &self,
        options: CloudDisconnectOptions,
    ) -> Result<CloudDisconnectReport, CloudConnectError> {
        self.set_desired_link(false, CloudCliLinkMode::Managed)?;
        let relay_url = options
            .relay_url_override
            .or_else(|| self.secrets.get_utf8(RELAY_URL_SECRET).ok().flatten());
        let oauth_token = self.load_oauth_token()?;

        let relay_outcome = match (options.environment_id, relay_url, oauth_token) {
            (Some(env_id), Some(base), Some(tok)) => {
                revoke_remote_environment_link(&self.http, &base, &env_id, &tok.access_token).await
            }
            _ => RelayUnlinkOutcome::NotAuthenticated,
        };

        for key in PERSISTED_CLOUD_LINK_SECRETS {
            let _ = self.secrets.remove(key)?;
        }
        if options.clear_authorization {
            let _ = self.clear_oauth_token()?;
        }

        Ok(CloudDisconnectReport {
            relay_outcome,
            cleared_authorization: options.clear_authorization,
        })
    }
}
