//! RFC 8628 OAuth device-code authorization and relay unlink HTTP client helpers.

use std::time::{Duration, Instant};

use reqwest::Client;
use reqwest::header::{AUTHORIZATION, HeaderName, HeaderValue};
use serde::Deserialize;

use super::types::{
    CloudConnectError, DEVICE_CODE_GRANT_TYPE, DeviceAuthorizationConfig,
    DeviceAuthorizationPrompt, PersistedCloudCliToken, RelayUnlinkOutcome, current_epoch_ms,
};

#[derive(Deserialize)]
struct OAuthTokenSuccess {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    expires_in: i64,
    #[serde(default)]
    id_token: Option<String>,
}

#[derive(Deserialize)]
struct OAuthTokenErrorBody {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

#[derive(Deserialize)]
struct RelayOkResponse {
    ok: bool,
}

/// Calls OAuth `POST /oauth/device_authorization` (`CliTokenManager.ts:447-456`).
pub(super) async fn request_device_authorization(
    http: &Client,
    cfg: &DeviceAuthorizationConfig,
) -> Result<DeviceAuthorizationPrompt, CloudConnectError> {
    let scope = cfg.scopes.join(" ");
    let resp = http
        .post(&cfg.device_authorization_endpoint)
        .form(&[
            ("client_id", cfg.client_id.as_str()),
            ("scope", scope.as_str()),
        ])
        .timeout(cfg.request_timeout)
        .send()
        .await
        .map_err(|err| CloudConnectError::Authorization {
            detail: format!("device authorization request failed: {err}"),
        })?;
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let detail = resp.text().await.unwrap_or_default();
        return Err(CloudConnectError::Authorization {
            detail: format!("device authorization returned HTTP {status}: {detail}"),
        });
    }
    resp.json::<DeviceAuthorizationPrompt>()
        .await
        .map_err(|err| CloudConnectError::Authorization {
            detail: format!("invalid device authorization response JSON: {err}"),
        })
}

/// Polls RFC 8628 `POST /oauth/token` until authorized, denied, or expired (`CliTokenManager.ts:394-441`).
pub(super) async fn poll_device_authorization_token(
    http: &Client,
    cfg: &DeviceAuthorizationConfig,
    device_code: &str,
    initial_interval: Duration,
    expires_in: Duration,
) -> Result<PersistedCloudCliToken, CloudConnectError> {
    let deadline = Instant::now() + expires_in;
    let mut interval = initial_interval;
    let params = [
        ("grant_type", DEVICE_CODE_GRANT_TYPE),
        ("device_code", device_code),
        ("client_id", cfg.client_id.as_str()),
    ];

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(CloudConnectError::AuthorizationTimeout);
        }
        tokio::time::sleep(interval.min(remaining)).await;
        if Instant::now() >= deadline {
            return Err(CloudConnectError::AuthorizationTimeout);
        }

        let Ok(resp) = http
            .post(&cfg.token_endpoint)
            .form(&params)
            .timeout(cfg.request_timeout)
            .send()
            .await
        else {
            interval = interval.saturating_add(cfg.slow_down_increment);
            continue;
        };

        let status = resp.status();
        if status.is_server_error() {
            let _ = resp.bytes().await;
            interval = interval.saturating_add(cfg.slow_down_increment);
            continue;
        }
        if status.is_success() {
            let body = resp.json::<OAuthTokenSuccess>().await.map_err(|err| {
                CloudConnectError::Authorization {
                    detail: format!("invalid OAuth token response JSON: {err}"),
                }
            })?;
            return Ok(PersistedCloudCliToken {
                access_token: body.access_token,
                refresh_token: body.refresh_token.unwrap_or_default(),
                expires_at_epoch_ms: current_epoch_ms()
                    .saturating_add(body.expires_in.saturating_mul(1_000)),
                identity: body.id_token,
            });
        }

        let err_body = resp.json::<OAuthTokenErrorBody>().await.map_err(|err| {
            CloudConnectError::Authorization {
                detail: format!("invalid OAuth error response JSON: {err}"),
            }
        })?;
        match err_body.error.as_str() {
            "authorization_pending" => {}
            "slow_down" => {
                interval = interval.saturating_add(cfg.slow_down_increment);
            }
            "expired_token" => return Err(CloudConnectError::AuthorizationTimeout),
            "access_denied" => return Err(CloudConnectError::AuthorizationDenied),
            other => {
                return Err(CloudConnectError::Authorization {
                    detail: err_body
                        .error_description
                        .unwrap_or_else(|| other.to_owned()),
                });
            }
        }
    }
}

/// Calls `DELETE /v1/client/environment-links/<environment_id>` on the Legion Connect relay.
pub(super) async fn revoke_remote_environment_link(
    http: &Client,
    relay_url: &str,
    environment_id: &str,
    access_token: &str,
) -> RelayUnlinkOutcome {
    let trimmed = relay_url.trim_end_matches('/');
    let url = format!("{trimmed}/v1/client/environment-links/{environment_id}");
    let Ok(hdr_val) = HeaderValue::from_str(&format!("Bearer {access_token}")) else {
        return RelayUnlinkOutcome::Failed("invalid bearer token header".to_owned());
    };
    let resp = match http
        .delete(&url)
        .header(AUTHORIZATION, hdr_val)
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(err) => return RelayUnlinkOutcome::Failed(err.to_string()),
    };
    let _ = HeaderName::from_static("authorization");
    if !resp.status().is_success() {
        return RelayUnlinkOutcome::Failed(format!("HTTP {}", resp.status().as_u16()));
    }
    match resp.json::<RelayOkResponse>().await {
        Ok(body) if body.ok => RelayUnlinkOutcome::Revoked,
        Ok(_) => RelayUnlinkOutcome::NotLinked,
        Err(err) => RelayUnlinkOutcome::Failed(err.to_string()),
    }
}
