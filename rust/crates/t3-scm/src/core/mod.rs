//! Core source-control primitives shared across forge adapters.
//!
//! Includes remote URL discovery, per-host rate limiting with a 10% quota
//! reserve policy, merge commit message and check rollup helpers, and `SHA-256`
//! file-revision tracking.

pub mod discovery;
pub(crate) mod http_quota;
pub mod pr_helpers;
pub mod rate_limit;
pub mod revisions;

use thiserror::Error;

/// Errors raised by shared `core` source-control utilities.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CoreError {
    /// Failed to parse or validate a git remote URL.
    #[error("invalid git remote `{raw}`: {reason}")]
    InvalidRemote {
        /// Original remote input string.
        raw: String,
        /// Explanation of why parsing failed.
        reason: String,
    },
    /// Rate limit or quota headroom prevented acquiring a request permit.
    #[error("rate limited for host `{host}`: {reason}")]
    RateLimited {
        /// Normalized destination hostname.
        host: String,
        /// Cause of rejection or backoff window.
        reason: String,
    },
    /// Invalid check, patch path, or file revision payload.
    #[error("invalid revision or patch payload: {0}")]
    InvalidRevision(String),
}
