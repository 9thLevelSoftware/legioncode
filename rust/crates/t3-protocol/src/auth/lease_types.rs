//! Data types and typed errors for [`super::SessionLeaseRegistry`].

use t3_contracts::permissions::EnvironmentScope;
use thiserror::Error;

/// Persisted session lease record tracked by [`super::SessionLeaseRegistry`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLease {
    /// Unique server session identifier.
    pub session_id: String,
    /// Authenticated principal subject.
    pub subject: String,
    /// Granted environment scopes.
    pub scopes: Vec<EnvironmentScope>,
    /// RFC 7638 JWK thumbprint for `DPoP`-bound sessions, if any.
    pub proof_key_thumbprint: Option<String>,
    /// Whether this lease can be extended or rotated via `DPoP` renewal.
    pub renewable: bool,
    /// Issuance timestamp in epoch milliseconds.
    pub issued_at_ms: i64,
    /// Authoritative database expiration timestamp in epoch milliseconds.
    pub db_expires_at_ms: i64,
    /// Timestamp when the session was revoked, if revoked.
    pub revoked_at_ms: Option<i64>,
    /// Immediate predecessor session ID if rotated from an earlier lease.
    pub predecessor_id: Option<String>,
    /// Immediate successor session ID if rotated to a newer lease.
    pub successor_id: Option<String>,
    /// Optional human-readable device or client label.
    pub label: Option<String>,
}

/// Parameters for registering a newly issued [`SessionLease`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSessionLease {
    /// Unique server session identifier.
    pub session_id: String,
    /// Authenticated principal subject.
    pub subject: String,
    /// Granted environment scopes.
    pub scopes: Vec<EnvironmentScope>,
    /// Optional RFC 7638 JWK thumbprint binding.
    pub proof_key_thumbprint: Option<String>,
    /// Whether `DPoP` renewal is enabled.
    pub renewable: bool,
    /// Issuance timestamp in epoch milliseconds.
    pub issued_at_ms: i64,
    /// Database expiration timestamp in epoch milliseconds.
    pub db_expires_at_ms: i64,
    /// Optional human-readable client label.
    pub label: Option<String>,
}

/// Parameters for renewing a `DPoP`-bound session lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenewDpopSessionInput {
    /// Existing session identifier being renewed.
    pub session_id: String,
    /// Thumbprint verified from the caller's incoming `DPoP` proof.
    pub presented_thumbprint: String,
    /// When `Some`, rotates into a new successor lease with this ID and migrates live sockets.
    pub new_session_id: Option<String>,
    /// Extended database lease expiration timestamp in epoch milliseconds.
    pub new_expires_at_ms: i64,
    /// Current wall-clock timestamp in epoch milliseconds.
    pub now_ms: i64,
}

/// Reason a tracked WebSocket connection is no longer valid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionInvalidationReason {
    /// Connection was explicitly closed.
    Disconnected,
    /// Current lease or an ancestor in its renewal chain was revoked.
    Revoked {
        /// Session ID whose revocation terminated the chain.
        session_id: String,
    },
    /// Active database lease reached `db_expires_at_ms`.
    Expired {
        /// Session ID whose lease expired.
        session_id: String,
        /// Expiration timestamp in epoch milliseconds.
        expired_at_ms: i64,
    },
    /// Backing lease record could not be found.
    MissingLease {
        /// Missing active session identifier.
        session_id: String,
    },
}

/// Current validity state of a tracked socket connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    /// Socket remains authorized under `session_id` until `expires_at_ms`.
    Active {
        /// Currently active lease identifier (after any renewal migrations).
        session_id: String,
        /// Authoritative database expiration timestamp in epoch milliseconds.
        expires_at_ms: i64,
    },
    /// Socket must be closed or rejected.
    Invalidated(ConnectionInvalidationReason),
}

/// Active session summary returned by [`super::SessionLeaseRegistry::list_active_sessions`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveSessionSummary {
    /// Session lease record.
    pub lease: SessionLease,
    /// Number of currently connected sockets anchored on this exact session ID.
    pub connection_count: usize,
    /// True when at least one live socket is currently anchored on this session ID.
    pub connected: bool,
}

/// Failure raised by session verification or connection admission.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SessionVerificationError {
    /// No lease matches `session_id`.
    #[error("session {session_id:?} not found")]
    NotFound {
        /// Unrecognized session identifier.
        session_id: String,
    },
    /// Lease (or an ancestor in its renewal chain) has been revoked.
    #[error("session {session_id:?} was revoked at {revoked_at_ms}")]
    Revoked {
        /// Revoked session identifier.
        session_id: String,
        /// Revocation timestamp in epoch milliseconds.
        revoked_at_ms: i64,
    },
    /// Token `exp` claim is at or before `now_ms`.
    #[error("session token expired at {expired_at_ms}")]
    TokenExpired {
        /// Token expiration timestamp in epoch milliseconds.
        expired_at_ms: i64,
    },
    /// Authoritative database lease `db_expires_at_ms` is at or before `now_ms`.
    #[error("database session lease expired at {expired_at_ms}")]
    DatabaseLeaseExpired {
        /// Database expiration timestamp in epoch milliseconds.
        expired_at_ms: i64,
    },
}

/// Failure raised by `DPoP` session renewal.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SessionRenewalError {
    /// No lease matches `session_id`.
    #[error("session {session_id:?} not found")]
    NotFound {
        /// Missing session identifier.
        session_id: String,
    },
    /// Lease is marked non-renewable.
    #[error("session {session_id:?} is not renewable")]
    NotRenewable {
        /// Session identifier.
        session_id: String,
    },
    /// Lease is not bound to a `DPoP` JWK thumbprint.
    #[error("session {session_id:?} lacks a DPoP proof key binding")]
    MissingProofBinding {
        /// Session identifier.
        session_id: String,
    },
    /// Presented `DPoP` key thumbprint does not match the lease binding.
    #[error("DPoP proof key thumbprint does not match session {session_id:?}")]
    ThumbprintMismatch {
        /// Session identifier.
        session_id: String,
    },
    /// Lease (or an ancestor) has already been revoked.
    #[error("session {session_id:?} was revoked")]
    Revoked {
        /// Revoked session identifier.
        session_id: String,
    },
    /// Underlying database lease already expired (`db_expires_at_ms <= now_ms`).
    #[error("database session lease expired at {expired_at_ms}")]
    DatabaseLeaseExpired {
        /// Expiration timestamp in epoch milliseconds.
        expired_at_ms: i64,
    },
    /// Lease was already rotated to a successor session.
    #[error("session was already renewed to successor {successor_id:?}")]
    AlreadySuperseded {
        /// Existing successor session identifier.
        successor_id: String,
    },
    /// Supplied successor expiry is not strictly in the future relative to `now_ms`.
    #[error("new expiry {new_expires_at_ms} must exceed now {now_ms}")]
    InvalidExpiry {
        /// Proposed database expiration timestamp.
        new_expires_at_ms: i64,
        /// Current timestamp.
        now_ms: i64,
    },
    /// Supplied `new_session_id` collides with an existing lease.
    #[error("successor session ID {session_id:?} already exists")]
    DuplicateSessionId {
        /// Colliding successor ID.
        session_id: String,
    },
}

/// Default sliding lifetime extended on active peer-link sessions (30 days in ms).
pub const PEER_LINK_SESSION_TTL_MS: i64 = 30 * 24 * 60 * 60 * 1_000;

/// Minimum remaining expiry delta required before persisting a peer-link renewal (1 day in ms).
pub const PEER_LINK_RENEWAL_STEP_MS: i64 = 24 * 60 * 60 * 1_000;

/// Outcome of touching an active linked-environment session lease (`f6d941fa4f` / #39399).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerLeaseTouch {
    /// Current lease state after the touch (`db_expires_at_ms` advances if renewed).
    pub lease: SessionLease,
    /// `true` when `db_expires_at_ms` was extended by at least [`PEER_LINK_RENEWAL_STEP_MS`].
    pub renewed: bool,
}
