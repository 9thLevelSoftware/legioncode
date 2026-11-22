//! `GitCafe` adapter with strict host allowlisting, same-origin `/api/` checks, and revision-fenced review drafts.

use http::Method;
use legion_contracts::pull_requests::{PullRequestChecksState, PullRequestMergeability};
use serde_json::{Value, json};

use crate::forges::{
    ForgeError, HttpRequestSpec, NormalizedPullRequest, is_same_origin, json_str, json_u64,
    parse_standard_pr_state, validate_hostname,
};

/// Strict `GitCafe` hostname allowlist (no arbitrary self-hosted hosts allowed).
pub const ALLOWED_GITCAFE_HOSTS: [&str; 2] = ["git.cafe", "staging.git.cafe"];

/// Revision fence attached to mutable `GitCafe` PR actions and review drafts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCafeRevisionFence {
    /// Expected monotonic PR version (`expectedVersion`).
    pub expected_version: i64,
    /// Expected head commit `OID` (`commitOid`).
    pub commit_oid: String,
    /// Idempotency request identifier (`requestId`).
    pub request_id: String,
}

/// Diff side targeted by an inline `GitCafe` review comment draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewSide {
    /// Base/deletion side (`LEFT`).
    Left,
    /// Head/addition side (`RIGHT`).
    Right,
}

impl ReviewSide {
    /// Wire representation expected by `GitCafe` review draft endpoints.
    #[must_use]
    pub const fn as_wire_str(self) -> &'static str {
        match self {
            Self::Left => "LEFT",
            Self::Right => "RIGHT",
        }
    }
}

/// Revision-fenced inline review comment draft payload for `GitCafe`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCafeReviewDraft {
    /// Repository-relative file path.
    pub path: String,
    /// 1-based line number on `side`.
    pub line: u32,
    /// Target diff side.
    pub side: ReviewSide,
    /// Comment markdown body.
    pub body: String,
    /// Revision fence verified before submitting or persisting drafts.
    pub fence: GitCafeRevisionFence,
}

/// Returns `true` only when `host` is explicitly listed in [`ALLOWED_GITCAFE_HOSTS`].
#[must_use]
pub fn is_allowed_gitcafe_host(host: &str) -> bool {
    let Ok(normalized) = validate_hostname(host) else {
        return false;
    };
    ALLOWED_GITCAFE_HOSTS.contains(&normalized.as_str())
}

/// Resolves the canonical `HTTPS` origin for an allowlisted `GitCafe` host.
///
/// # Errors
///
/// Returns [`ForgeError::InvalidHost`] if `host` is not in [`ALLOWED_GITCAFE_HOSTS`].
///
/// ```
/// use legion_scm::forges::gitcafe::resolve_gitcafe_origin;
///
/// assert_eq!(resolve_gitcafe_origin("git.cafe")?, "https://git.cafe");
/// assert_eq!(resolve_gitcafe_origin("staging.git.cafe")?, "https://staging.git.cafe");
/// assert!(resolve_gitcafe_origin("evil.git.cafe").is_err());
/// # Ok::<(), legion_scm::forges::ForgeError>(())
/// ```
pub fn resolve_gitcafe_origin(host: &str) -> Result<&'static str, ForgeError> {
    let normalized = validate_hostname(host)?;
    match normalized.as_str() {
        "git.cafe" => Ok("https://git.cafe"),
        "staging.git.cafe" => Ok("https://staging.git.cafe"),
        _ => Err(ForgeError::InvalidHost {
            host: host.to_owned(),
            reason: "host is outside the strict GitCafe allowlist".to_owned(),
        }),
    }
}

/// Validates that `candidate_url` shares `origin` and begins with `/api/`.
///
/// # Errors
///
/// Returns [`ForgeError::UntrustedUrl`] on origin mismatch or path outside `/api/`.
pub fn validate_gitcafe_api_url(origin: &str, candidate_url: &str) -> Result<String, ForgeError> {
    let trimmed = candidate_url.trim();
    if trimmed.starts_with('/') {
        if !trimmed.starts_with("/api/") {
            return Err(ForgeError::UntrustedUrl(trimmed.to_owned()));
        }
        return Ok(format!("{origin}{trimmed}"));
    }
    if !is_same_origin(origin, trimmed) {
        return Err(ForgeError::UntrustedUrl(trimmed.to_owned()));
    }
    let path_after_origin = trimmed.strip_prefix(origin).unwrap_or("");
    if !path_after_origin.starts_with("/api/") {
        return Err(ForgeError::UntrustedUrl(trimmed.to_owned()));
    }
    Ok(trimmed.to_owned())
}

/// Verifies a client [`GitCafeRevisionFence`] against current server PR state.
///
/// # Errors
///
/// Returns [`ForgeError::RevisionConflict`] if either `expected_version != server_version`
/// or `commit_oid != server_commit_oid`, or [`ForgeError::MalformedResponse`] if request metadata
/// is empty.
pub fn verify_gitcafe_revision_fence(
    fence: &GitCafeRevisionFence,
    server_version: i64,
    server_commit_oid: &str,
) -> Result<(), ForgeError> {
    if fence.request_id.trim().is_empty() || fence.commit_oid.trim().is_empty() {
        return Err(ForgeError::MalformedResponse(
            "GitCafe revision fence requires non-empty commit_oid and request_id".to_owned(),
        ));
    }
    if fence.expected_version != server_version || fence.commit_oid != server_commit_oid {
        return Err(ForgeError::RevisionConflict {
            expected_version: fence.expected_version,
            expected_commit: fence.commit_oid.clone(),
            actual_version: server_version,
            actual_commit: server_commit_oid.to_owned(),
        });
    }
    Ok(())
}

/// Builds a revision-fenced `POST /api/...` review-draft request after checking the fence and URL.
///
/// # Errors
///
/// Returns [`ForgeError`] if host validation, token validation, or fence verification fails.
pub fn build_gitcafe_review_draft_request(
    host: &str,
    token: &str,
    api_path: &str,
    draft: &GitCafeReviewDraft,
    server_version: i64,
    server_commit_oid: &str,
) -> Result<HttpRequestSpec, ForgeError> {
    let origin = resolve_gitcafe_origin(host)?;
    verify_gitcafe_revision_fence(&draft.fence, server_version, server_commit_oid)?;
    let url = validate_gitcafe_api_url(origin, api_path)?;

    let clean_token = token.trim();
    if clean_token.is_empty() || clean_token.contains(['\r', '\n']) {
        return Err(ForgeError::InvalidCredentials(
            "GitCafe token is empty or contains line breaks".to_owned(),
        ));
    }
    let payload = json!({
        "path": draft.path,
        "line": draft.line,
        "side": draft.side.as_wire_str(),
        "body": draft.body,
        "expectedVersion": draft.fence.expected_version,
        "commitOid": draft.fence.commit_oid,
        "requestId": draft.fence.request_id,
    });
    Ok(HttpRequestSpec {
        method: Method::POST,
        url,
        headers: vec![
            ("authorization".to_owned(), format!("Bearer {clean_token}")),
            ("accept".to_owned(), "application/json".to_owned()),
            ("content-type".to_owned(), "application/json".to_owned()),
        ],
        body: Some(payload.to_string()),
    })
}

/// Normalizes a `GitCafe` pull request JSON payload into [`NormalizedPullRequest`].
///
/// # Errors
///
/// Returns [`ForgeError::MalformedResponse`] if required fields (`number`, `title`, branches) are absent.
pub fn normalize_gitcafe_pr_json(json: &Value) -> Result<NormalizedPullRequest, ForgeError> {
    let number = json_u64(json, "number")
        .ok_or_else(|| ForgeError::MalformedResponse("missing GitCafe PR `number`".to_owned()))?;
    let title = json_str(json, "title")
        .ok_or_else(|| ForgeError::MalformedResponse("missing GitCafe PR `title`".to_owned()))?
        .to_owned();
    let url = json_str(json, "url")
        .or_else(|| json_str(json, "htmlUrl"))
        .unwrap_or("")
        .to_owned();

    let is_merged = json.get("merged").and_then(Value::as_bool) == Some(true)
        || json_str(json, "state").is_some_and(|s| s.eq_ignore_ascii_case("merged"));
    let state = parse_standard_pr_state(json_str(json, "state").unwrap_or("open"), is_merged);
    let is_draft = json
        .get("isDraft")
        .and_then(Value::as_bool)
        .or_else(|| json.get("draft").and_then(Value::as_bool))
        .unwrap_or(false);

    let head_branch = json_str(json, "headRef")
        .or_else(|| json.get("head").and_then(|h| json_str(h, "ref")))
        .ok_or_else(|| ForgeError::MalformedResponse("missing GitCafe PR head branch".to_owned()))?
        .to_owned();
    let base_branch = json_str(json, "baseRef")
        .or_else(|| json.get("base").and_then(|b| json_str(b, "ref")))
        .ok_or_else(|| ForgeError::MalformedResponse("missing GitCafe PR base branch".to_owned()))?
        .to_owned();
    let head_sha = json_str(json, "commitOid")
        .or_else(|| json.get("head").and_then(|h| json_str(h, "sha")))
        .map(str::to_owned);

    let mergeability = match json.get("mergeable").and_then(Value::as_bool) {
        Some(true) => PullRequestMergeability::Mergeable,
        Some(false) => PullRequestMergeability::Conflicting,
        None => PullRequestMergeability::Unknown,
    };

    Ok(NormalizedPullRequest {
        number,
        title,
        url,
        state,
        is_draft,
        head_branch,
        base_branch,
        head_sha,
        mergeability,
        checks_state: PullRequestChecksState::Pending,
    })
}
