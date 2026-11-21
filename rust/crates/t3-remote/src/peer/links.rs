//! Peer environment link persistence, URL security validation, and CLI formatters.
//!
//! Mirrors `apps/server/src/peer/PeerLinks.ts` and `apps/server/src/cli/environment.ts:71-84,134,155-158`.

#[path = "links_format.rs"]
mod format;
#[path = "links_types.rs"]
mod types;
#[path = "links_url.rs"]
mod url_guard;

use std::collections::HashSet;

use rusqlite::Connection;
use t3_persistence::auth_records::{
    PeerLinkRecord, insert_peer_link, list_peer_links, unlink_peer_link,
};
use uuid::Uuid;

use crate::secrets::FileSecretStore;

pub use format::{
    PeerLinkStatus, PeerLinkSummary, format_peer_links, format_peer_links_json,
    format_peer_unlink_result,
};
pub use types::{
    InsertPeerLinkInput, PeerLink, PeerLinkError, PeerLinkErrorReason, StoredPeerCredentials,
};
use types::{record_to_link, serialize_access, storage_err};
pub use url_guard::validate_peer_origin;

/// Peer-link persistence service combining SQLite `peer_environment_links` and [`FileSecretStore`].
///
/// Guarantees token-before-row insertion with secret cleanup on SQLite failure, and ensures
/// [`Self::unlink`] deletes **both** the SQLite row and the `.bin` secret file (`PeerLinks.ts:623-634`).
///
/// # Examples
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use t3_contracts::ids::EnvironmentId;
/// use t3_contracts::orchestration::McpClientAccess;
/// use t3_persistence::sqlite::SqliteStore;
/// use t3_remote::peer::links::{InsertPeerLinkInput, PeerLinksService};
/// use t3_remote::secrets::FileSecretStore;
///
/// let dir = tempfile::tempdir()?;
/// let store = SqliteStore::open_in_memory()?;
/// let secrets = FileSecretStore::from_state_dir(dir.path())?;
/// let service = PeerLinksService::new(secrets);
///
/// let linked = service.insert(store.connection(), InsertPeerLinkInput {
///     environment_id: EnvironmentId::new("env-box")?,
///     label: "Box".to_owned(),
///     urls: vec!["https://box.tail.ts.net/".to_owned()],
///     access: McpClientAccess::Auto,
///     access_token: "secret-tok".to_owned(),
///     linked_at: "2026-11-20T00:00:00.000Z".to_owned(),
///     expires_at: "2026-12-20T00:00:00.000Z".to_owned(),
/// })?;
/// assert_eq!(linked.urls, vec!["https://box.tail.ts.net"]);
/// assert!(service.unlink(store.connection(), "env-box")?);
/// assert!(!service.unlink(store.connection(), "env-box")?);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct PeerLinksService {
    secrets: FileSecretStore,
}

impl PeerLinksService {
    /// Creates a [`PeerLinksService`] backed by `secrets`.
    #[must_use]
    pub fn new(secrets: FileSecretStore) -> Self {
        Self { secrets }
    }

    /// Returns a reference to the underlying [`FileSecretStore`].
    #[must_use]
    pub fn secrets(&self) -> &FileSecretStore {
        &self.secrets
    }

    /// Lists all persisted peer links ordered by `label ASC, environment_id ASC`.
    ///
    /// # Errors
    ///
    /// Returns [`PeerLinkError`] with [`PeerLinkErrorReason::Storage`] if SQLite or row decoding fails.
    pub fn list(&self, conn: &Connection) -> Result<Vec<PeerLink>, PeerLinkError> {
        let rows = list_peer_links(conn).map_err(storage_err)?;
        rows.into_iter().map(record_to_link).collect()
    }

    /// Lists peer links enriched with expiration checks and `probe_status` for live links.
    ///
    /// # Errors
    ///
    /// Returns [`PeerLinkError`] with [`PeerLinkErrorReason::Storage`] if reading peer rows fails.
    pub fn list_summaries(
        &self,
        conn: &Connection,
        now_iso: &str,
        probe_status: impl Fn(&PeerLink) -> PeerLinkStatus,
    ) -> Result<Vec<PeerLinkSummary>, PeerLinkError> {
        let links = self.list(conn)?;
        Ok(links
            .into_iter()
            .map(|link| {
                let status = if link.expires_at.as_str() <= now_iso {
                    PeerLinkStatus::Expired
                } else {
                    probe_status(&link)
                };
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
            })
            .collect())
    }

    /// Returns a single stored peer link by `environment_id`, or `Ok(None)` if unlinked.
    ///
    /// # Errors
    ///
    /// Returns [`PeerLinkError`] with [`PeerLinkErrorReason::Storage`] if SQLite reading or decoding fails.
    pub fn get(
        &self,
        conn: &Connection,
        environment_id: &str,
    ) -> Result<Option<PeerLink>, PeerLinkError> {
        let rows = list_peer_links(conn).map_err(storage_err)?;
        let found = rows
            .into_iter()
            .find(|r| r.environment_id == environment_id);
        found.map(record_to_link).transpose()
    }

    /// Validates URLs, stores the bearer secret in [`FileSecretStore`], and upserts the DB row.
    ///
    /// # Errors
    ///
    /// Returns [`PeerLinkError`] when URLs fail security validation or SQLite/secret storage fails.
    pub fn insert(
        &self,
        conn: &Connection,
        input: InsertPeerLinkInput,
    ) -> Result<PeerLink, PeerLinkError> {
        if input.urls.is_empty() {
            return Err(PeerLinkError::new(
                PeerLinkErrorReason::Unreachable,
                "At least one peer URL is required.",
            ));
        }
        let mut seen = HashSet::new();
        let mut origins = Vec::with_capacity(input.urls.len());
        for raw in &input.urls {
            let origin = validate_peer_origin(raw)?;
            if seen.insert(origin.clone()) {
                origins.push(origin);
            }
        }

        let previous_secret_name = list_peer_links(conn)
            .map_err(storage_err)?
            .into_iter()
            .find(|r| r.environment_id == input.environment_id.as_str())
            .map(|r| r.secret_name);

        let secret_name = format!("peer-link-{}", Uuid::new_v4().simple());
        self.secrets
            .set_utf8(&secret_name, &input.access_token)
            .map_err(storage_err)?;

        let access_str = serialize_access(input.access)?;
        let record = PeerLinkRecord {
            environment_id: input.environment_id.as_str().to_owned(),
            secret_name: secret_name.clone(),
            label: input.label.clone(),
            urls: origins.clone(),
            access: access_str,
            linked_at: input.linked_at.clone(),
            expires_at: input.expires_at.clone(),
            last_reached_at: Some(input.linked_at.clone()),
            last_error: None,
            fenced: true,
        };

        if let Err(err) = insert_peer_link(conn, &record) {
            let _ = self.secrets.remove(&secret_name);
            return Err(storage_err(err));
        }

        if let Some(prev) = previous_secret_name
            && prev != secret_name
        {
            let _ = self.secrets.remove(&prev);
        }

        Ok(PeerLink {
            environment_id: input.environment_id,
            label: input.label,
            urls: origins,
            access: input.access,
            linked_at: input.linked_at.clone(),
            expires_at: input.expires_at,
            last_reached_at: Some(input.linked_at),
            last_error: None,
        })
    }

    /// Resolves link metadata and its stored bearer token, checking expiry (`PeerLinks.ts:636-659`).
    ///
    /// # Errors
    ///
    /// Returns [`PeerLinkError`] if the link is unknown, expired, missing its secret, or unreadable.
    pub fn resolve_credentials(
        &self,
        conn: &Connection,
        environment_id: &str,
        now_iso: &str,
    ) -> Result<StoredPeerCredentials, PeerLinkError> {
        let rows = list_peer_links(conn).map_err(storage_err)?;
        let Some(row) = rows
            .into_iter()
            .find(|r| r.environment_id == environment_id)
        else {
            return Err(PeerLinkError::new(
                PeerLinkErrorReason::UnknownLink,
                format!(
                    "This environment is not linked to {environment_id}. t3_environment_links lists the linked ones."
                ),
            ));
        };
        let secret_name = row.secret_name.clone();
        let link = record_to_link(row)?;
        if link.expires_at.as_str() <= now_iso {
            return Err(PeerLinkError::new(
                PeerLinkErrorReason::Expired,
                format!(
                    "The link to {} expired. Link it again with a new pairing code from {}.",
                    link.label, link.label
                ),
            ));
        }
        let token = self
            .secrets
            .get_utf8(&secret_name)
            .map_err(storage_err)?
            .ok_or_else(|| {
                PeerLinkError::new(
                    PeerLinkErrorReason::UnknownLink,
                    format!(
                        "The link to {} lost its credential. Link it again.",
                        link.label
                    ),
                )
            })?;
        Ok(StoredPeerCredentials { link, token })
    }

    /// Removes `environment_id` from SQLite AND deletes its `.bin` token file (`PeerLinks.ts:623-634`).
    ///
    /// # Errors
    ///
    /// Returns [`PeerLinkError`] with [`PeerLinkErrorReason::Storage`] if SQLite or secret deletion fails.
    pub fn unlink(&self, conn: &Connection, environment_id: &str) -> Result<bool, PeerLinkError> {
        let Some(secret_name) = unlink_peer_link(conn, environment_id).map_err(storage_err)? else {
            return Ok(false);
        };
        let _ = self.secrets.remove(&secret_name).map_err(storage_err)?;
        Ok(true)
    }
}
