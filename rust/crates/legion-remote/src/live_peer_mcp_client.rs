//! HTTP peer discovery, `/api/auth/pair` exchange, and live peer summary probing.

use std::collections::HashSet;
use std::time::Duration;

use futures_util::future::join_all;
use legion_contracts::brand::ACTIVE_BRAND;
use legion_contracts::environment::ExecutionEnvironmentDescriptor;
use reqwest::header::ACCEPT;
use reqwest::{Client, StatusCode};
use rusqlite::Connection;
use serde_json::json;

use super::types::{
    LivePeerLinkInput, NormalizedPairToken, PairTokenResponse, current_utc_iso, expires_utc_iso,
    map_link_storage_err,
};
use crate::peer::links::{
    InsertPeerLinkInput, PeerLink, PeerLinkError, PeerLinkErrorReason, PeerLinkStatus,
    PeerLinkSummary, PeerLinksService, validate_peer_origin,
};
use crate::secrets::FileSecretStore;

const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Async HTTP peer discovery, pairing exchange, and live status probe client.
///
/// Refuses HTTP redirects so pairing codes and bearer tokens only ever reach validated origins.
///
/// ```
/// use legion_remote::live_network::LivePeerClient;
///
/// let client = LivePeerClient::new()?;
/// assert!(format!("{client:?}").contains("LivePeerClient"));
/// # Ok::<(), legion_remote::peer::links::PeerLinkError>(())
/// ```
#[derive(Debug, Clone)]
pub struct LivePeerClient {
    http: Client,
    probe_timeout: Duration,
}

impl LivePeerClient {
    /// Creates a [`LivePeerClient`] with a non-redirecting `reqwest::Client`.
    pub fn new() -> Result<Self, PeerLinkError> {
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|err| {
                PeerLinkError::new(
                    PeerLinkErrorReason::Unreachable,
                    format!("Failed to initialize peer HTTP client: {err}"),
                )
            })?;
        Ok(Self {
            http,
            probe_timeout: DEFAULT_PROBE_TIMEOUT,
        })
    }

    /// Sets the timeout for individual `/.well-known/legioncode/environment` probes.
    #[must_use]
    pub fn with_probe_timeout(mut self, timeout: Duration) -> Self {
        self.probe_timeout = timeout;
        self
    }

    /// Validates `url` and probes `<origin>/.well-known/<brand>/environment`.
    pub async fn probe_environment(
        &self,
        url: &str,
    ) -> Result<(String, ExecutionEnvironmentDescriptor), PeerLinkError> {
        let origin = validate_peer_origin(url)?;
        let well_known_prefix = ACTIVE_BRAND.cli_bin_name;
        let resp = self
            .http
            .get(format!(
                "{origin}/.well-known/{well_known_prefix}/environment"
            ))
            .header(ACCEPT, "application/json")
            .timeout(self.probe_timeout)
            .send()
            .await
            .map_err(|err| {
                PeerLinkError::new(
                    PeerLinkErrorReason::Unreachable,
                    format!("{origin} did not answer: {err}"),
                )
            })?;
        if !resp.status().is_success() {
            return Err(PeerLinkError::new(
                PeerLinkErrorReason::NotALegionCodeEnvironment,
                format!(
                    "{origin} answered HTTP {}, not as a {} environment.",
                    resp.status().as_u16(),
                    ACTIVE_BRAND.product_name
                ),
            ));
        }
        let descriptor = resp
            .json::<ExecutionEnvironmentDescriptor>()
            .await
            .map_err(|_| {
                PeerLinkError::new(
                    PeerLinkErrorReason::NotALegionCodeEnvironment,
                    format!(
                        "{origin} answered, but not as a {} environment this version understands.",
                        ACTIVE_BRAND.product_name
                    ),
                )
            })?;
        Ok((origin, descriptor))
    }

    /// Probes `input.url`, exchanges `pairing_code` at `/api/auth/pair`, and persists via [`PeerLinksService`].
    pub async fn link_peer_over_http(
        &self,
        conn: &Connection,
        secrets: &FileSecretStore,
        input: LivePeerLinkInput,
    ) -> Result<PeerLink, PeerLinkError> {
        let (primary_origin, descriptor) = self.probe_environment(&input.url).await?;
        let mut origins = vec![primary_origin.clone()];
        let mut seen = HashSet::from([primary_origin.clone()]);
        for alt in &input.alternate_urls {
            let alt_origin = validate_peer_origin(alt)?;
            if seen.insert(alt_origin.clone()) {
                origins.push(alt_origin);
            }
        }
        if let Some(expected) = &input.expected_environment_id
            && descriptor.environment_id != *expected
        {
            return Err(map_link_storage_err(
                PeerLinkErrorReason::Unreachable,
                format!(
                    "{primary_origin} answered as another environment, {}.",
                    descriptor.label
                ),
            )
            .with_answered_as(descriptor.label));
        }
        if input.self_environment_id.as_ref() == Some(&descriptor.environment_id) {
            return Err(PeerLinkError::new(
                PeerLinkErrorReason::SelfLink,
                "That address is this environment.",
            ));
        }
        if input.require_link_limits
            && (descriptor.capabilities.mcp_mode_limit_header != Some(true)
                || descriptor.capabilities.link_fence != Some(true))
        {
            return Err(PeerLinkError::new(
                PeerLinkErrorReason::Incompatible,
                format!(
                    "{} runs a {} version that cannot keep a linked agent's limits. Update it, then link again.",
                    descriptor.label, ACTIVE_BRAND.product_name
                ),
            ));
        }

        let service = PeerLinksService::new(secrets.clone());
        let linked_at = input.linked_at.unwrap_or_else(current_utc_iso);
        if input.expected_environment_id.is_none()
            && let Some(existing) = service.get(conn, descriptor.environment_id.as_str())?
            && existing.expires_at.as_str() > linked_at.as_str()
            && !existing.urls.contains(&primary_origin)
        {
            return Err(PeerLinkError::new(
                PeerLinkErrorReason::PairingRejected,
                format!(
                    "{primary_origin} answers as {}, which is already linked at another address. If {} moved, forget that link first.",
                    existing.label, existing.label
                ),
            ));
        }

        let token = self
            .exchange_pairing_code(&primary_origin, &input.pairing_code, &input.client_label)
            .await?;
        let expires_at = token
            .expires_at
            .unwrap_or_else(|| expires_utc_iso(token.expires_in));

        service.insert(
            conn,
            InsertPeerLinkInput {
                environment_id: descriptor.environment_id,
                label: descriptor.label,
                urls: origins,
                access: input.access,
                access_token: token.access_token,
                linked_at,
                expires_at,
            },
        )
    }

    /// Probes every stored peer environment concurrently and returns live [`PeerLinkSummary`] rows.
    pub async fn list_live_peer_summaries(
        &self,
        conn: &Connection,
        secrets: &FileSecretStore,
        now_iso: &str,
    ) -> Result<Vec<PeerLinkSummary>, PeerLinkError> {
        let links = PeerLinksService::new(secrets.clone()).list(conn)?;
        let probes = join_all(links.into_iter().map(|link| async move {
            if link.expires_at.as_str() <= now_iso {
                return summary_with_status(link, PeerLinkStatus::Expired);
            }
            for url in &link.urls {
                if let Ok((_, desc)) = self.probe_environment(url).await
                    && desc.environment_id == link.environment_id
                {
                    let mut updated = link;
                    updated.label = desc.label;
                    return summary_with_status(updated, PeerLinkStatus::Reachable);
                }
            }
            summary_with_status(link, PeerLinkStatus::Unreachable)
        }))
        .await;
        Ok(probes)
    }

    async fn exchange_pairing_code(
        &self,
        origin: &str,
        pairing_code: &str,
        client_label: &str,
    ) -> Result<NormalizedPairToken, PeerLinkError> {
        let resp = self
            .http
            .post(format!("{origin}/api/auth/pair"))
            .header(ACCEPT, "application/json")
            .json(&json!({
                "token": pairing_code,
                "clientLabel": client_label,
            }))
            .timeout(self.probe_timeout)
            .send()
            .await
            .map_err(|err| {
                PeerLinkError::new(
                    PeerLinkErrorReason::Unreachable,
                    format!("{origin} stopped answering while linking: {err}"),
                )
            })?;

        let status = resp.status();
        if matches!(
            status,
            StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ) {
            let detail = resp.text().await.unwrap_or_default();
            return Err(PeerLinkError::new(
                PeerLinkErrorReason::PairingRejected,
                if detail.trim().is_empty() {
                    "The peer refused the pairing code.".to_owned()
                } else {
                    detail
                },
            ));
        }
        if !status.is_success() {
            return Err(PeerLinkError::new(
                PeerLinkErrorReason::Incompatible,
                format!("{origin} returned HTTP {} during pairing.", status.as_u16()),
            ));
        }
        let parsed = resp.json::<PairTokenResponse>().await.map_err(|_| {
            PeerLinkError::new(
                PeerLinkErrorReason::Incompatible,
                format!("{origin} answered in a shape this version does not understand."),
            )
        })?;
        parsed.into_normalized().ok_or_else(|| {
            PeerLinkError::new(
                PeerLinkErrorReason::Incompatible,
                format!("{origin} answered without a valid access token."),
            )
        })
    }
}

fn summary_with_status(link: PeerLink, status: PeerLinkStatus) -> PeerLinkSummary {
    PeerLinkSummary {
        environment_id: link.environment_id,
        label: link.label,
        urls: link.urls,
        access: link.access,
        linked_at: link.linked_at,
        expires_at: link.expires_at,
        last_reached_at: link.last_reached_at,
        last_error: link.last_error,
        status,
    }
}
