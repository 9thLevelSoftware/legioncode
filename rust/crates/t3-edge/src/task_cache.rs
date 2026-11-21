//! Cloudflare task-cache edge validation, isolate admission, D1 batch accounting, and OIDC
//! JWKS refresh cooldown (`BUG-30`).
//!
//! Mirrors `infra/task-cache/src/{Admission.ts,Cache.ts,NativeClients.ts,Savings.ts,Summary.ts}`
//! and `infra/task-cache/vendor/vite-plus-remote-cache-cloudflare/src/{admission.ts,auth.ts}`.
//!
//! # Examples
//!
//! ```
//! use t3_edge::task_cache::{
//!     AdmissionKind, CacheAdmissionPolicy, D1BatchLedger, D1LedgerOp, MAX_MULTIPART_PARTS,
//!     OIDC_JWKS_COOLDOWN_MS, OidcCooldownTracker, PartValidationError, validate_part_number,
//! };
//!
//! // BUG-30: validate multipart part numbers strictly in 1..=64 at PUT.
//! assert_eq!(validate_part_number(1), Ok(1));
//! assert_eq!(validate_part_number(MAX_MULTIPART_PARTS), Ok(64));
//! let err = validate_part_number(65).expect_err("part 65 rejected at PUT");
//! assert_eq!(err.http_status(), 400);
//! assert_eq!(err.error_code(), "invalid_part_number");
//! assert!(matches!(err, PartValidationError::InvalidPartNumber { part: 65, .. }));
//!
//! // Isolate admission permits 2 concurrent stores and 4 concurrent reads without queueing.
//! let mut admission = CacheAdmissionPolicy::new();
//! let mut p1 = admission.try_acquire(AdmissionKind::Store)?;
//! let mut p2 = admission.try_acquire(AdmissionKind::Store)?;
//! assert!(admission.try_acquire(AdmissionKind::Store).is_err());
//! assert!(admission.release(&mut p1));
//! assert!(!admission.release(&mut p1));
//! assert!(admission.release(&mut p2));
//!
//! // OIDC JWKS cooldown enforces a 30_000 ms floor even after failed refreshes.
//! let mut oidc = OidcCooldownTracker::new();
//! assert!(oidc.try_start_refresh(10_000).is_ok());
//! assert!(oidc.try_start_refresh(10_000 + OIDC_JWKS_COOLDOWN_MS - 1).is_err());
//! assert!(oidc.try_start_refresh(10_000 + OIDC_JWKS_COOLDOWN_MS).is_ok());
//!
//! // D1 batch ledger tracks hit/miss/eviction bytes atomically.
//! let mut ledger = D1BatchLedger::new(1024, 10, 10);
//! ledger.execute_batch(&[
//!     D1LedgerOp::Reserve { bytes: 256 },
//!     D1LedgerOp::Publish {
//!         key_is_new: true,
//!         association_is_new: false,
//!         actual_bytes: 200,
//!         reserved_bytes: 256,
//!         retired_bytes: None,
//!     },
//! ])?;
//! assert_eq!(ledger.charged_bytes(), 200);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use crate::task_cache_admission::{
    AdmissionKind, AdmissionPermit, AdmissionRejected, CacheAdmissionPolicy,
    DEFAULT_MAX_CONCURRENT_READS, DEFAULT_MAX_CONCURRENT_STORES, DeploymentQuotaState,
    ScopeQuotaState, WriteQuotaCheckInput, WriteQuotaError,
};
pub use crate::task_cache_ledger::D1BatchLedger;
pub use crate::task_cache_ledger_types::{
    D1BatchError, D1BatchReceipt, D1LedgerOp, DEFAULT_ASSOCIATION_LIMIT, DEFAULT_BYTE_LIMIT,
    DEFAULT_ENTRY_LIMIT,
};
pub use crate::task_cache_savings::{
    SavingsKey, SavingsTotals, SummaryRecord, TaskSavingsEntry, TaskSavingsSource,
};

/// Maximum multipart upload part count allowed by `NativeClients` (`NativeClients.ts:35`).
pub const MAX_MULTIPART_PARTS: u32 = 64;

/// Maximum JSON body size accepted by `POST .../complete` (`64 KiB`, `NativeClients.ts:171`).
pub const MAX_COMPLETE_BODY_BYTES: usize = 64 * 1024;

/// Minimum interval in milliseconds between upstream GitHub OIDC JWKS refreshes (`auth.ts:24,27`).
pub const OIDC_JWKS_COOLDOWN_MS: u64 = 30_000;

/// Error returned when a `PUT` part number or `POST .../complete` part list fails validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
pub enum PartValidationError {
    /// Part number was `0` or exceeded `MAX_MULTIPART_PARTS` (`64`) at `PUT` (`BUG-30`).
    #[error("invalid multipart part number {part}: expected 1..={max_parts}")]
    InvalidPartNumber {
        /// Submitted part number.
        part: u32,
        /// Inclusive upper bound (`MAX_MULTIPART_PARTS`).
        max_parts: u32,
    },
    /// Raw URL segment did not represent a canonical positive integer (`1..=64`).
    #[error("invalid multipart part segment '{segment}'")]
    InvalidPartSegment {
        /// Unparsed URL path segment.
        segment: String,
    },
    /// Complete request included zero parts or more than `MAX_MULTIPART_PARTS`.
    #[error("invalid complete part count {count}: expected 1..={max_parts}")]
    InvalidPartCount {
        /// Number of submitted parts.
        count: usize,
        /// Inclusive upper bound (`MAX_MULTIPART_PARTS`).
        max_parts: u32,
    },
    /// Complete request repeated a part number.
    #[error("duplicate multipart part number {part}")]
    DuplicatePartNumber {
        /// Repeated part number.
        part: u32,
    },
    /// Complete request omitted an `ETag` for a part.
    #[error("empty etag for multipart part {part}")]
    EmptyEtag {
        /// Part number with empty `ETag`.
        part: u32,
    },
}

impl PartValidationError {
    /// HTTP status code returned to clients (`400 Bad Request`).
    pub fn http_status(&self) -> u16 {
        400
    }

    /// Machine-readable JSON error reason (`"invalid_part_number"` for part validation failures).
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::InvalidPartNumber { .. } | Self::InvalidPartSegment { .. } => {
                "invalid_part_number"
            }
            Self::InvalidPartCount { .. }
            | Self::DuplicatePartNumber { .. }
            | Self::EmptyEtag { .. } => "invalid_parts",
        }
    }
}

/// Validates a multipart `PUT` part number in `1..=MAX_MULTIPART_PARTS` (`BUG-30`).
///
/// Rejects `0` and any value `> 64` at `PUT` time with `PartValidationError::InvalidPartNumber`
/// (`400 invalid_part_number`), closing the legacy gap where `65..=99` were accepted on `PUT`
/// and rejected only at `POST .../complete`.
pub fn validate_part_number(part: u32) -> Result<u32, PartValidationError> {
    if part == 0 || part > MAX_MULTIPART_PARTS {
        return Err(PartValidationError::InvalidPartNumber {
            part,
            max_parts: MAX_MULTIPART_PARTS,
        });
    }
    Ok(part)
}

/// Parses and validates a raw URL path segment for a multipart `PUT` part.
pub fn parse_part_number(segment: &str) -> Result<u32, PartValidationError> {
    if segment.is_empty()
        || segment.starts_with('0')
        || !segment.bytes().all(|b| b.is_ascii_digit())
    {
        if segment == "0" {
            return Err(PartValidationError::InvalidPartNumber {
                part: 0,
                max_parts: MAX_MULTIPART_PARTS,
            });
        }
        return Err(PartValidationError::InvalidPartSegment {
            segment: segment.to_owned(),
        });
    }
    let part = segment
        .parse::<u32>()
        .map_err(|_| PartValidationError::InvalidPartSegment {
            segment: segment.to_owned(),
        })?;
    validate_part_number(part)
}

/// Uploaded part descriptor submitted to `POST .../complete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletedPart {
    /// One-based multipart part number (`1..=64`).
    pub part_number: u32,
    /// R2 `ETag` returned by `PUT`.
    pub etag: String,
}

/// Validates the full list of parts passed to `POST .../complete` (`NativeClients.ts:31-38`).
pub fn validate_complete_parts(parts: &[CompletedPart]) -> Result<(), PartValidationError> {
    if parts.is_empty() || parts.len() > MAX_MULTIPART_PARTS as usize {
        return Err(PartValidationError::InvalidPartCount {
            count: parts.len(),
            max_parts: MAX_MULTIPART_PARTS,
        });
    }
    let mut seen = BTreeSet::new();
    for part in parts {
        let num = validate_part_number(part.part_number)?;
        if !seen.insert(num) {
            return Err(PartValidationError::DuplicatePartNumber { part: num });
        }
        if part.etag.trim().is_empty() {
            return Err(PartValidationError::EmptyEtag { part: num });
        }
    }
    Ok(())
}

/// Error returned when an OIDC JWKS refresh is attempted inside the `30_000 ms` cooldown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("OIDC JWKS refresh cooldown active for {retry_after_ms}ms")]
pub struct OidcCooldownError {
    /// Remaining cooldown duration in milliseconds.
    pub retry_after_ms: u64,
}

impl OidcCooldownError {
    /// HTTP status code (`503 Service Unavailable`).
    pub fn http_status(self) -> u16 {
        503
    }

    /// Machine-readable error code (`"unavailable"`).
    pub fn error_code(self) -> &'static str {
        "unavailable"
    }
}

/// Tracks the `30_000 ms` GitHub Actions OIDC JWKS refresh cooldown (`auth.ts:18-45`).
///
/// Unlike `CloudflareApiPauseGate` (which must not advance `checked_at_ms` on failure), failed
/// JWKS fetches **do** engage the `30_000 ms` cooldown so Worker isolates do not storm GitHub's
/// JWKS endpoint during an outage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OidcCooldownTracker {
    cooldown_ms: u64,
    last_attempt_ms: Option<u64>,
}

impl Default for OidcCooldownTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl OidcCooldownTracker {
    /// Creates a tracker with the default `30_000 ms` cooldown.
    pub fn new() -> Self {
        Self::with_cooldown_ms(OIDC_JWKS_COOLDOWN_MS)
    }

    /// Creates a tracker with a custom cooldown window in milliseconds.
    pub fn with_cooldown_ms(cooldown_ms: u64) -> Self {
        Self {
            cooldown_ms,
            last_attempt_ms: None,
        }
    }

    /// Returns the timestamp of the last recorded JWKS refresh attempt, if any.
    pub fn last_attempt_ms(&self) -> Option<u64> {
        self.last_attempt_ms
    }

    /// Returns remaining cooldown milliseconds at `now_ms` (`0` when ready).
    pub fn remaining_cooldown_ms(&self, now_ms: u64) -> u64 {
        match self.last_attempt_ms {
            None => 0,
            Some(last) => {
                let elapsed = now_ms.saturating_sub(last);
                self.cooldown_ms.saturating_sub(elapsed)
            }
        }
    }

    /// Attempts to start a JWKS refresh at `now_ms`, engaging the `30_000 ms` cooldown window.
    pub fn try_start_refresh(&mut self, now_ms: u64) -> Result<(), OidcCooldownError> {
        let remaining = self.remaining_cooldown_ms(now_ms);
        if remaining > 0 {
            return Err(OidcCooldownError {
                retry_after_ms: remaining,
            });
        }
        self.last_attempt_ms = Some(now_ms);
        Ok(())
    }
}
