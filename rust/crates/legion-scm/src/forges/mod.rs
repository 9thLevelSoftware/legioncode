//! Source-control forge and issue-tracker adapters (`GitHub`, `GitLab`, `Azure` `DevOps`,
//! `Bitbucket`, `Forgejo`, `GitCafe`, `Cursor` `Origin`, and `Linear`).
//!
//! Each submodule provides typed HTTP/CLI request builders, rate-limit or revision
//! safety guards, and response normalizers producing `legion-contracts` types.

pub mod azure_devops;
pub mod azure_devops_auth;
pub mod bitbucket;
pub mod cursor_origin;
pub mod forgejo;
pub mod forgejo_template;
pub mod gitcafe;
pub mod github;
pub mod github_checks;
pub mod github_graphql;
pub mod gitlab;
pub mod issue_links;
pub mod linear;
pub mod linear_accounts;

use http::Method;
use legion_contracts::pull_requests::{
    PullRequestChecksState, PullRequestMergeability, PullRequestState,
};
use serde_json::Value;
use thiserror::Error;

/// Shared errors returned by forge request builders and response normalizers.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ForgeError {
    /// Invalid or untrusted hostname or origin.
    #[error("invalid or untrusted forge host `{host}`: {reason}")]
    InvalidHost {
        /// Offending host or URL value.
        host: String,
        /// Validation failure explanation.
        reason: String,
    },
    /// Rejected redirect or cross-origin URL.
    #[error("untrusted cross-origin or path URL `{0}`")]
    UntrustedUrl(String),
    /// Invalid API credentials or header injection characters.
    #[error("invalid credentials: {0}")]
    InvalidCredentials(String),
    /// Upstream refused request due to expired/missing authentication.
    #[error("authentication required: {0}")]
    AuthRequired(String),
    /// Revision fence (`expectedVersion` / `commitOid`) conflicted with server state.
    #[error(
        "revision conflict (expected v{expected_version}/{expected_commit}, actual v{actual_version}/{actual_commit})"
    )]
    RevisionConflict {
        /// Client expected monotonic revision.
        expected_version: i64,
        /// Client expected commit OID.
        expected_commit: String,
        /// Server current revision.
        actual_version: i64,
        /// Server current commit OID.
        actual_commit: String,
    },
    /// Upstream rate-limit or quota threshold triggered.
    #[error("forge rate limited: {0}")]
    RateLimited(String),
    /// Malformed or incomplete forge response payload.
    #[error("malformed forge response: {0}")]
    MalformedResponse(String),
}

/// Typed outbound HTTP request specification built by pure Rust forge clients.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequestSpec {
    /// HTTP request method (`GET`, `POST`, `PATCH`, `PUT`, `DELETE`).
    pub method: Method,
    /// Full destination URL.
    pub url: String,
    /// Ordered `(header_name, header_value)` pairs.
    pub headers: Vec<(String, String)>,
    /// Optional serialized request payload (typically JSON).
    pub body: Option<String>,
}

/// Typed CLI invocation specification (`gh`, `glab`, `az`/`python.exe`, `tea`, `origin`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliCommandSpec {
    /// Executable or absolute interpreter path.
    pub program: String,
    /// Ordered argument list passed without shell interpolation.
    pub args: Vec<String>,
    /// Explicit environment variable overrides (`GITLAB_HOST`, `GH_HOST`, `AZ_INSTALLER`).
    pub env: Vec<(String, String)>,
    /// Optional `stdin` content (e.g. `@-` body delivery on `Windows` / `Snap`).
    pub stdin: Option<String>,
}

/// Normalized pull/merge request metadata produced by all 7 source-control forge adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedPullRequest {
    /// Numeric pull or merge request identifier.
    pub number: u64,
    /// Pull request title.
    pub title: String,
    /// Canonical web URL.
    pub url: String,
    /// Normalized lifecycle state (`Open`, `Closed`, `Merged`).
    pub state: PullRequestState,
    /// Whether the PR is marked as draft or WIP.
    pub is_draft: bool,
    /// Source branch name (without `refs/heads/`).
    pub head_branch: String,
    /// Target/base branch name (without `refs/heads/`).
    pub base_branch: String,
    /// Latest head commit `SHA`, if available.
    pub head_sha: Option<String>,
    /// Mergeability assessment.
    pub mergeability: PullRequestMergeability,
    /// Aggregate CI check state.
    pub checks_state: PullRequestChecksState,
}

/// Validates and lowercases a bare hostname (no scheme, path, whitespace, or `@`).
///
/// # Errors
///
/// Returns [`ForgeError::InvalidHost`] when `raw` is empty, contains scheme/path/whitespace,
/// or starts with `-`.
pub fn validate_hostname(raw: &str) -> Result<String, ForgeError> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('-')
        || trimmed.contains("://")
        || trimmed.contains('/')
        || trimmed.contains('?')
        || trimmed.contains('#')
        || trimmed.contains('@')
        || trimmed
            .bytes()
            .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
    {
        return Err(ForgeError::InvalidHost {
            host: raw.to_owned(),
            reason: "must be a clean hostname without scheme, path, `@`, or whitespace".to_owned(),
        });
    }
    Ok(trimmed.to_ascii_lowercase())
}

/// Checks whether `candidate_url` shares the exact scheme and authority (`host[:port]`) of `base_url`.
#[must_use]
pub fn is_same_origin(base_url: &str, candidate_url: &str) -> bool {
    match (origin_tuple(base_url), origin_tuple(candidate_url)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

fn origin_tuple(url: &str) -> Option<(String, String)> {
    let (scheme, rest) = url.trim().split_once("://")?;
    let scheme_lower = scheme.to_ascii_lowercase();
    if scheme_lower != "https" && scheme_lower != "http" {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    Some((scheme_lower, authority.to_ascii_lowercase()))
}

/// Encodes arbitrary bytes to standard padded `Base64` (`RFC 4648`).
#[must_use]
pub fn encode_base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = if chunk.len() > 1 {
            u32::from(chunk[1])
        } else {
            0
        };
        let b2 = if chunk.len() > 2 {
            u32::from(chunk[2])
        } else {
            0
        };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(char::from(
            TABLE[usize::try_from((triple >> 18) & 0x3f).unwrap_or(0)],
        ));
        out.push(char::from(
            TABLE[usize::try_from((triple >> 12) & 0x3f).unwrap_or(0)],
        ));
        if chunk.len() > 1 {
            out.push(char::from(
                TABLE[usize::try_from((triple >> 6) & 0x3f).unwrap_or(0)],
            ));
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(char::from(
                TABLE[usize::try_from(triple & 0x3f).unwrap_or(0)],
            ));
        } else {
            out.push('=');
        }
    }
    out
}

pub(crate) fn json_str<'a>(val: &'a Value, key: &str) -> Option<&'a str> {
    val.get(key).and_then(Value::as_str)
}

pub(crate) fn json_u64(val: &Value, key: &str) -> Option<u64> {
    val.get(key).and_then(Value::as_u64)
}

pub(crate) fn parse_standard_pr_state(raw: &str, is_merged: bool) -> PullRequestState {
    if is_merged {
        return PullRequestState::Merged;
    }
    match raw.trim().to_ascii_lowercase().as_str() {
        "open" | "opened" | "active" => PullRequestState::Open,
        "merged" => PullRequestState::Merged,
        _ => PullRequestState::Closed,
    }
}
