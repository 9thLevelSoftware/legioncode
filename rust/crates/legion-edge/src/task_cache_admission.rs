//! Isolate concurrency permits and D1 scope/deployment write quota admission checks.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Maximum concurrent store requests allowed per Worker isolate (`Admission.ts:25`).
pub const DEFAULT_MAX_CONCURRENT_STORES: usize = 2;

/// Maximum concurrent read requests allowed per Worker isolate (`Admission.ts:26`).
pub const DEFAULT_MAX_CONCURRENT_READS: usize = 4;

/// Request classification for isolate concurrency admission (`Admission.ts:16-19`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AdmissionKind {
    /// Cache write / multipart upload request (capped at 2 per isolate).
    Store,
    /// Cache read / blob download / telemetry request (capped at 4 per isolate).
    Read,
}

/// Owned concurrency permit issued by [`CacheAdmissionPolicy::try_acquire`].
#[derive(Debug, PartialEq, Eq)]
pub struct AdmissionPermit {
    kind: AdmissionKind,
    released: bool,
}

impl AdmissionPermit {
    /// Operation category held by this permit.
    pub fn kind(&self) -> AdmissionKind {
        self.kind
    }

    /// Returns `true` if [`CacheAdmissionPolicy::release`] has already consumed this permit.
    pub fn is_released(&self) -> bool {
        self.released
    }
}

/// Error returned immediately when all isolate admission permits for a kind are busy (`503`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("isolate admission concurrency limit reached ({kind:?})")]
pub struct AdmissionRejected {
    /// Operation kind that ran out of permits.
    pub kind: AdmissionKind,
}

impl AdmissionRejected {
    /// HTTP status code (`503 Service Unavailable`).
    pub fn http_status(self) -> u16 {
        503
    }

    /// Machine-readable error code (`"concurrency_limit"`).
    pub fn error_code(self) -> &'static str {
        "concurrency_limit"
    }
}

/// Reasons a cache write reservation is denied by scope or deployment policy (`0001_cache.sql:97-105`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum WriteQuotaError {
    /// Deployment or scope is disabled or has writes turned off.
    #[error("cache writes are disabled by scope or deployment policy")]
    WritesDisabled,
    /// OIDC / developer token expired at `now_secs`.
    #[error("writer authorization token has expired")]
    TokenExpired,
    /// Uploaded policy version no longer matches the active scope policy version.
    #[error("stale scope policy version")]
    PolicyVersionMismatch,
    /// Scope or deployment byte ceiling would be exceeded by `incoming_bytes`.
    #[error("cache byte quota exceeded")]
    ByteQuotaExceeded,
}

/// Global deployment flags and quota counters (`deployment` table in `0001_cache.sql`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeploymentQuotaState {
    /// Whether the global deployment is enabled.
    pub enabled: bool,
    /// Whether global deployment writes are enabled.
    pub writes_enabled: bool,
    /// Global charged bytes currently stored.
    pub charged_bytes: u64,
    /// Global deployment byte limit.
    pub byte_limit: u64,
}

/// Scope-level flags and quota counters (`scopes` table in `0001_cache.sql`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScopeQuotaState {
    /// Whether the target scope is enabled.
    pub enabled: bool,
    /// Whether target scope writes are enabled.
    pub writes_enabled: bool,
    /// Current scope charged bytes.
    pub charged_bytes: u64,
    /// Scope byte limit.
    pub byte_limit: u64,
    /// Current scope `policy_version`.
    pub policy_version: u64,
}

/// Snapshot of D1 scope & deployment flags checked during write admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteQuotaCheckInput {
    /// Deployment-wide quota and enablement state.
    pub deployment: DeploymentQuotaState,
    /// Target scope quota and policy version state.
    pub scope: ScopeQuotaState,
    /// Captured `policy_version` on the upload reservation.
    pub expected_policy_version: u64,
    /// Token expiration epoch seconds (`token_exp`).
    pub token_exp_secs: u64,
}

/// Non-blocking isolate concurrency and quota admission policy (`Admission.ts` & `0001_cache.sql`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheAdmissionPolicy {
    max_stores: usize,
    max_reads: usize,
    active_stores: usize,
    active_reads: usize,
}

impl Default for CacheAdmissionPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl CacheAdmissionPolicy {
    /// Creates an admission policy with `2` store permits and `4` read permits.
    pub fn new() -> Self {
        Self::with_limits(DEFAULT_MAX_CONCURRENT_STORES, DEFAULT_MAX_CONCURRENT_READS)
    }

    /// Creates an admission policy with custom store and read concurrency caps.
    pub fn with_limits(max_stores: usize, max_reads: usize) -> Self {
        Self {
            max_stores,
            max_reads,
            active_stores: 0,
            active_reads: 0,
        }
    }

    /// Currently active store permits.
    pub fn active_stores(&self) -> usize {
        self.active_stores
    }

    /// Currently active read permits.
    pub fn active_reads(&self) -> usize {
        self.active_reads
    }

    /// Attempts to acquire a permit without queueing across Worker request boundaries.
    pub fn try_acquire(
        &mut self,
        kind: AdmissionKind,
    ) -> Result<AdmissionPermit, AdmissionRejected> {
        match kind {
            AdmissionKind::Store => {
                if self.active_stores >= self.max_stores {
                    return Err(AdmissionRejected { kind });
                }
                self.active_stores += 1;
            }
            AdmissionKind::Read => {
                if self.active_reads >= self.max_reads {
                    return Err(AdmissionRejected { kind });
                }
                self.active_reads += 1;
            }
        }
        Ok(AdmissionPermit {
            kind,
            released: false,
        })
    }

    /// Releases a previously acquired [`AdmissionPermit`]; subsequent calls with the same
    /// permit are no-ops and return `false`.
    pub fn release(&mut self, permit: &mut AdmissionPermit) -> bool {
        if permit.released {
            return false;
        }
        permit.released = true;
        match permit.kind {
            AdmissionKind::Store => {
                self.active_stores = self.active_stores.saturating_sub(1);
            }
            AdmissionKind::Read => {
                self.active_reads = self.active_reads.saturating_sub(1);
            }
        }
        true
    }

    /// Evaluates `reserve_capacity` SQL trigger preconditions (`0001_cache.sql:97-105`).
    pub fn check_write_quota(
        input: &WriteQuotaCheckInput,
        incoming_bytes: u64,
        now_secs: u64,
    ) -> Result<(), WriteQuotaError> {
        if !input.deployment.enabled
            || !input.deployment.writes_enabled
            || !input.scope.enabled
            || !input.scope.writes_enabled
        {
            return Err(WriteQuotaError::WritesDisabled);
        }
        if input.scope.policy_version != input.expected_policy_version {
            return Err(WriteQuotaError::PolicyVersionMismatch);
        }
        if input.token_exp_secs <= now_secs {
            return Err(WriteQuotaError::TokenExpired);
        }
        let next_scope = input.scope.charged_bytes.checked_add(incoming_bytes);
        let next_deploy = input.deployment.charged_bytes.checked_add(incoming_bytes);
        match (next_scope, next_deploy) {
            (Some(s), Some(d))
                if s <= input.scope.byte_limit && d <= input.deployment.byte_limit =>
            {
                Ok(())
            }
            _ => Err(WriteQuotaError::ByteQuotaExceeded),
        }
    }
}
