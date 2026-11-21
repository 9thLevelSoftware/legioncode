//! Server-side session lease verification, `DPoP` successor renewals, and socket invalidation.
//!
//! Fixes BUG-04 in `apps/server/src/auth/SessionStore.ts` and `ws.ts`:
//! - [`SessionLeaseRegistry::verify_session`] enforces `db_expires_at_ms > now_ms`
//!   in addition to the token claim expiration and revocation status.
//! - [`SessionLeaseRegistry::renew_dpop_session`] tracks `successor_id` renewal
//!   chains, migrates live WebSocket connections onto the active successor, keeps
//!   ancestor revocation transitive across the chain, and reports superseded
//!   predecessors as disconnected once migrated.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use tokio::sync::Notify;

use super::lease_state::{LiveConnection, RegistryState};
use super::lease_types::{
    ActiveSessionSummary, ConnectionInvalidationReason, ConnectionState, NewSessionLease,
    RenewDpopSessionInput, SessionLease, SessionRenewalError, SessionVerificationError,
};

/// Thread-safe registry tracking session leases, `DPoP` renewals, and socket connections.
#[derive(Debug, Clone, Default)]
pub struct SessionLeaseRegistry {
    state: Arc<RwLock<RegistryState>>,
    notify: Arc<Notify>,
}

impl SessionLeaseRegistry {
    /// Creates an empty [`SessionLeaseRegistry`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a newly issued [`SessionLease`], returning `Err` on duplicate `session_id`.
    pub fn register(&self, input: NewSessionLease) -> Result<SessionLease, SessionRenewalError> {
        let mut guard = self.state.write().unwrap_or_else(PoisonError::into_inner);
        if guard.leases.contains_key(&input.session_id) {
            return Err(SessionRenewalError::DuplicateSessionId {
                session_id: input.session_id,
            });
        }
        let lease = SessionLease {
            session_id: input.session_id.clone(),
            subject: input.subject,
            scopes: input.scopes,
            proof_key_thumbprint: input.proof_key_thumbprint,
            renewable: input.renewable,
            issued_at_ms: input.issued_at_ms,
            db_expires_at_ms: input.db_expires_at_ms,
            revoked_at_ms: None,
            predecessor_id: None,
            successor_id: None,
            label: input.label,
        };
        guard.leases.insert(input.session_id, lease.clone());
        Ok(lease)
    }

    /// Verifies token expiry, lease existence, chain revocation, and `db_expires_at_ms > now_ms`.
    pub fn verify_session(
        &self,
        session_id: &str,
        token_expires_at_ms: i64,
        now_ms: i64,
    ) -> Result<SessionLease, SessionVerificationError> {
        if token_expires_at_ms <= now_ms {
            return Err(SessionVerificationError::TokenExpired {
                expired_at_ms: token_expires_at_ms,
            });
        }
        let guard = self.state.read().unwrap_or_else(PoisonError::into_inner);
        let lease =
            guard
                .leases
                .get(session_id)
                .ok_or_else(|| SessionVerificationError::NotFound {
                    session_id: session_id.to_owned(),
                })?;
        if let Some((revoked_id, revoked_at_ms)) = guard.find_revocation_in_ancestors(session_id) {
            return Err(SessionVerificationError::Revoked {
                session_id: revoked_id,
                revoked_at_ms,
            });
        }
        if lease.db_expires_at_ms <= now_ms {
            return Err(SessionVerificationError::DatabaseLeaseExpired {
                expired_at_ms: lease.db_expires_at_ms,
            });
        }
        Ok(lease.clone())
    }

    /// Renews a `DPoP` session in place or rotates to `new_session_id`, migrating sockets.
    pub fn renew_dpop_session(
        &self,
        input: &RenewDpopSessionInput,
    ) -> Result<SessionLease, SessionRenewalError> {
        let mut guard = self.state.write().unwrap_or_else(PoisonError::into_inner);
        let current = guard.validate_renewal_target(input)?;
        let updated = match input.new_session_id.as_deref() {
            None => {
                let entry = guard.leases.get_mut(&input.session_id).ok_or_else(|| {
                    SessionRenewalError::NotFound {
                        session_id: input.session_id.clone(),
                    }
                })?;
                entry.db_expires_at_ms = input.new_expires_at_ms;
                entry.clone()
            }
            Some(successor_id) => guard.apply_successor_rotation(current, successor_id, input),
        };
        drop(guard);
        self.notify.notify_waiters();
        Ok(updated)
    }

    /// Registers a live WebSocket connection on `session_id`.
    pub fn connect(&self, session_id: &str, now_ms: i64) -> Result<u64, SessionVerificationError> {
        let mut guard = self.state.write().unwrap_or_else(PoisonError::into_inner);
        let lease =
            guard
                .leases
                .get(session_id)
                .ok_or_else(|| SessionVerificationError::NotFound {
                    session_id: session_id.to_owned(),
                })?;
        if let Some((revoked_id, revoked_at_ms)) = guard.find_revocation_in_ancestors(session_id) {
            return Err(SessionVerificationError::Revoked {
                session_id: revoked_id,
                revoked_at_ms,
            });
        }
        if lease.db_expires_at_ms <= now_ms {
            return Err(SessionVerificationError::DatabaseLeaseExpired {
                expired_at_ms: lease.db_expires_at_ms,
            });
        }
        let lineage = guard.collect_ancestor_lineage(session_id);
        guard.next_connection_id = guard.next_connection_id.saturating_add(1);
        let conn_id = guard.next_connection_id;
        guard.connections.insert(
            conn_id,
            LiveConnection {
                active_session_id: session_id.to_owned(),
                lineage_session_ids: lineage,
            },
        );
        Ok(conn_id)
    }

    /// Removes a specific connection handle without disturbing other migrated handles.
    pub fn disconnect(&self, connection_id: u64) -> bool {
        let removed = self
            .state
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .connections
            .remove(&connection_id)
            .is_some();
        if removed {
            self.notify.notify_waiters();
        }
        removed
    }

    /// Revokes `session_id` and all forward successors, waking invalidated connection waiters.
    pub fn revoke_session(
        &self,
        session_id: &str,
        now_ms: i64,
    ) -> Result<usize, SessionVerificationError> {
        let mut guard = self.state.write().unwrap_or_else(PoisonError::into_inner);
        if !guard.leases.contains_key(session_id) {
            return Err(SessionVerificationError::NotFound {
                session_id: session_id.to_owned(),
            });
        }
        let mut affected = HashSet::new();
        let mut cursor = Some(session_id.to_owned());
        while let Some(id) = cursor {
            if !affected.insert(id.clone()) {
                break;
            }
            cursor = if let Some(lease) = guard.leases.get_mut(&id) {
                if lease.revoked_at_ms.is_none() {
                    lease.revoked_at_ms = Some(now_ms);
                }
                lease.successor_id.clone()
            } else {
                None
            };
        }
        let impacted = guard
            .connections
            .values()
            .filter(|conn| {
                conn.lineage_session_ids
                    .iter()
                    .any(|id| affected.contains(id))
            })
            .count();
        drop(guard);
        self.notify.notify_waiters();
        Ok(impacted)
    }

    /// Evaluates whether `connection_id` remains valid at `now_ms`.
    pub fn check_connection(&self, connection_id: u64, now_ms: i64) -> ConnectionState {
        let guard = self.state.read().unwrap_or_else(PoisonError::into_inner);
        let Some(conn) = guard.connections.get(&connection_id) else {
            return ConnectionState::Invalidated(ConnectionInvalidationReason::Disconnected);
        };
        if let Some((revoked_id, _)) = guard.find_revocation_in_ancestors(&conn.active_session_id) {
            return ConnectionState::Invalidated(ConnectionInvalidationReason::Revoked {
                session_id: revoked_id,
            });
        }
        for ancestor_id in &conn.lineage_session_ids {
            if let Some(lease) = guard.leases.get(ancestor_id)
                && lease.revoked_at_ms.is_some()
            {
                return ConnectionState::Invalidated(ConnectionInvalidationReason::Revoked {
                    session_id: ancestor_id.clone(),
                });
            }
        }
        let Some(lease) = guard.leases.get(&conn.active_session_id) else {
            return ConnectionState::Invalidated(ConnectionInvalidationReason::MissingLease {
                session_id: conn.active_session_id.clone(),
            });
        };
        if lease.db_expires_at_ms <= now_ms {
            return ConnectionState::Invalidated(ConnectionInvalidationReason::Expired {
                session_id: lease.session_id.clone(),
                expired_at_ms: lease.db_expires_at_ms,
            });
        }
        ConnectionState::Active {
            session_id: lease.session_id.clone(),
            expires_at_ms: lease.db_expires_at_ms,
        }
    }

    /// Lists non-revoked, non-expired leases paired with live socket connection counts.
    pub fn list_active_sessions(&self, now_ms: i64) -> Vec<ActiveSessionSummary> {
        let guard = self.state.read().unwrap_or_else(PoisonError::into_inner);
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for conn in guard.connections.values() {
            *counts.entry(conn.active_session_id.as_str()).or_insert(0) += 1;
        }
        let mut items: Vec<_> = guard
            .leases
            .values()
            .filter(|lease| {
                lease.db_expires_at_ms > now_ms
                    && guard
                        .find_revocation_in_ancestors(&lease.session_id)
                        .is_none()
            })
            .map(|lease| {
                let count = counts.get(lease.session_id.as_str()).copied().unwrap_or(0);
                ActiveSessionSummary {
                    lease: lease.clone(),
                    connection_count: count,
                    connected: count > 0,
                }
            })
            .collect();
        items.sort_by(|a, b| a.lease.session_id.cmp(&b.lease.session_id));
        items
    }

    /// Looks up a lease by `session_id`.
    pub fn get_lease(&self, session_id: &str) -> Option<SessionLease> {
        self.state
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .leases
            .get(session_id)
            .cloned()
    }

    /// Resolves only when `connection_id` is disconnected, revoked, or expires (no polling).
    pub async fn await_invalidation(
        &self,
        connection_id: u64,
        now_ms: impl Fn() -> i64,
    ) -> ConnectionInvalidationReason {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            match self.check_connection(connection_id, now_ms()) {
                ConnectionState::Invalidated(reason) => return reason,
                ConnectionState::Active { expires_at_ms, .. } => {
                    let delay_ms = expires_at_ms
                        .saturating_sub(now_ms())
                        .max(0)
                        .cast_unsigned();
                    let sleep = tokio::time::sleep(Duration::from_millis(delay_ms));
                    tokio::select! {
                        () = &mut notified => {}
                        () = sleep => {}
                    }
                }
            }
        }
    }
}
