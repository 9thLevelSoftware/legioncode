//! Internal state backing [`super::SessionLeaseRegistry`].

use std::collections::{HashMap, HashSet};

use super::lease_types::{RenewDpopSessionInput, SessionLease, SessionRenewalError};

#[derive(Debug, Clone)]
pub(super) struct LiveConnection {
    pub active_session_id: String,
    pub lineage_session_ids: HashSet<String>,
}

#[derive(Debug, Default)]
pub(super) struct RegistryState {
    pub leases: HashMap<String, SessionLease>,
    pub connections: HashMap<u64, LiveConnection>,
    pub next_connection_id: u64,
}

impl RegistryState {
    pub fn find_revocation_in_ancestors(&self, session_id: &str) -> Option<(String, i64)> {
        let mut cursor = Some(session_id.to_owned());
        let mut visited = HashSet::new();
        while let Some(id) = cursor.take() {
            if !visited.insert(id.clone()) {
                break;
            }
            let lease = self.leases.get(&id)?;
            if let Some(revoked_at) = lease.revoked_at_ms {
                return Some((id, revoked_at));
            }
            cursor.clone_from(&lease.predecessor_id);
        }
        None
    }

    pub fn collect_ancestor_lineage(&self, session_id: &str) -> HashSet<String> {
        let mut lineage = HashSet::new();
        let mut cursor = Some(session_id.to_owned());
        while let Some(id) = cursor.take() {
            if !lineage.insert(id.clone()) {
                break;
            }
            cursor = self
                .leases
                .get(&id)
                .and_then(|item| item.predecessor_id.clone());
        }
        lineage
    }

    pub fn validate_renewal_target(
        &self,
        input: &RenewDpopSessionInput,
    ) -> Result<SessionLease, SessionRenewalError> {
        if input.new_expires_at_ms <= input.now_ms {
            return Err(SessionRenewalError::InvalidExpiry {
                new_expires_at_ms: input.new_expires_at_ms,
                now_ms: input.now_ms,
            });
        }
        if let Some(successor_id) = &input.new_session_id
            && self.leases.contains_key(successor_id)
        {
            return Err(SessionRenewalError::DuplicateSessionId {
                session_id: successor_id.clone(),
            });
        }
        if let Some((revoked_id, _)) = self.find_revocation_in_ancestors(&input.session_id) {
            return Err(SessionRenewalError::Revoked {
                session_id: revoked_id,
            });
        }
        let current = self.leases.get(&input.session_id).cloned().ok_or_else(|| {
            SessionRenewalError::NotFound {
                session_id: input.session_id.clone(),
            }
        })?;
        if !current.renewable {
            return Err(SessionRenewalError::NotRenewable {
                session_id: input.session_id.clone(),
            });
        }
        let expected = current.proof_key_thumbprint.as_deref().ok_or_else(|| {
            SessionRenewalError::MissingProofBinding {
                session_id: input.session_id.clone(),
            }
        })?;
        if expected != input.presented_thumbprint {
            return Err(SessionRenewalError::ThumbprintMismatch {
                session_id: input.session_id.clone(),
            });
        }
        if current.db_expires_at_ms <= input.now_ms {
            return Err(SessionRenewalError::DatabaseLeaseExpired {
                expired_at_ms: current.db_expires_at_ms,
            });
        }
        if let Some(successor_id) = current.successor_id {
            return Err(SessionRenewalError::AlreadySuperseded { successor_id });
        }
        Ok(current)
    }

    pub fn apply_successor_rotation(
        &mut self,
        current: SessionLease,
        successor_id: &str,
        input: &RenewDpopSessionInput,
    ) -> SessionLease {
        let successor = SessionLease {
            session_id: successor_id.to_owned(),
            subject: current.subject,
            scopes: current.scopes,
            proof_key_thumbprint: current.proof_key_thumbprint,
            renewable: current.renewable,
            issued_at_ms: input.now_ms,
            db_expires_at_ms: input.new_expires_at_ms,
            revoked_at_ms: None,
            predecessor_id: Some(input.session_id.clone()),
            successor_id: None,
            label: current.label,
        };
        if let Some(pred) = self.leases.get_mut(&input.session_id) {
            pred.successor_id = Some(successor_id.to_owned());
        }
        self.leases
            .insert(successor_id.to_owned(), successor.clone());
        for conn in self.connections.values_mut() {
            if conn.active_session_id == input.session_id {
                successor_id.clone_into(&mut conn.active_session_id);
                conn.lineage_session_ids.insert(successor_id.to_owned());
            }
        }
        successor
    }
}
