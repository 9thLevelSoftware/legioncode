//! `Bitbucket` Cloud v2.0 REST adapter (`CONCURRENCY = 4`, Bearer/Basic auth, redirect checks).

use http::Method;
use serde_json::Value;
use t3_contracts::pull_requests::{
    PullRequestChecksState, PullRequestMergeability, PullRequestState,
};

use crate::forges::{
    ForgeError, HttpRequestSpec, NormalizedPullRequest, encode_base64, is_same_origin, json_str,
    json_u64,
};

/// Canonical `Bitbucket` Cloud REST v2.0 base URL.
pub const BITBUCKET_API_BASE: &str = "https://api.bitbucket.org/2.0";

/// Maximum concurrent requests recommended for `Bitbucket` v2.0 endpoints.
pub const BITBUCKET_CONCURRENCY: usize = 4;

/// Supported authentication methods for `Bitbucket` Cloud v2.0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BitbucketAuth {
    /// OAuth/Workspace/Repository access token sent as `Bearer <token>`.
    Bearer(String),
    /// Atlassian account email + API token encoded as HTTP Basic authentication.
    Basic {
        /// Account email address.
        email: String,
        /// Atlassian `Bitbucket` API token.
        api_token: String,
    },
}

impl BitbucketAuth {
    /// Formats the `Authorization` header value after validating printable ASCII bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ForgeError::InvalidCredentials`] if credentials are empty or contain spaces,
    /// control characters, or non-ASCII bytes.
    ///
    /// ```
    /// use t3_scm::forges::bitbucket::BitbucketAuth;
    ///
    /// let basic = BitbucketAuth::Basic {
    ///     email: "dev@example.com".into(),
    ///     api_token: "tok_123".into(),
    /// };
    /// let header = basic.authorization_header()?;
    /// assert!(header.starts_with("Basic "));
    /// # Ok::<(), t3_scm::forges::ForgeError>(())
    /// ```
    pub fn authorization_header(&self) -> Result<String, ForgeError> {
        match self {
            Self::Bearer(token) => {
                let clean = validate_printable_secret(token, "Bitbucket bearer token")?;
                Ok(format!("Bearer {clean}"))
            }
            Self::Basic { email, api_token } => {
                let clean_email = validate_printable_secret(email, "Bitbucket email")?;
                if clean_email.contains(':') {
                    return Err(ForgeError::InvalidCredentials(
                        "Bitbucket Basic email must not contain `:`".to_owned(),
                    ));
                }
                let clean_token = validate_printable_secret(api_token, "Bitbucket API token")?;
                let combined = format!("{clean_email}:{clean_token}");
                Ok(format!("Basic {}", encode_base64(combined.as_bytes())))
            }
        }
    }
}

/// Validates target URL origin when following relative endpoints or paginated `next` links.
///
/// # Errors
///
/// Returns [`ForgeError::UntrustedUrl`] if an absolute `next` URL points outside [`BITBUCKET_API_BASE`],
/// or [`ForgeError::InvalidCredentials`] if `auth` contains unsafe characters.
pub fn build_bitbucket_request(
    auth: &BitbucketAuth,
    method: Method,
    path_or_next_url: &str,
    body: Option<Value>,
) -> Result<HttpRequestSpec, ForgeError> {
    let url = resolve_bitbucket_url(path_or_next_url)?;
    let auth_header = auth.authorization_header()?;
    let mut headers = vec![
        ("authorization".to_owned(), auth_header),
        ("accept".to_owned(), "application/json".to_owned()),
    ];
    let body_str = body.map(|val| {
        headers.push(("content-type".to_owned(), "application/json".to_owned()));
        val.to_string()
    });
    Ok(HttpRequestSpec {
        method,
        url,
        headers,
        body: body_str,
    })
}

/// Ensures paginated or redirect URLs stay on [`BITBUCKET_API_BASE`]'s origin and `/2.0/` prefix.
///
/// # Errors
///
/// Returns [`ForgeError::UntrustedUrl`] for cross-origin, credential-bearing, or non-`/2.0/` URLs.
pub fn resolve_bitbucket_url(path_or_url: &str) -> Result<String, ForgeError> {
    let trimmed = path_or_url.trim();
    if trimmed.is_empty() {
        return Err(ForgeError::UntrustedUrl("empty Bitbucket path".to_owned()));
    }
    if trimmed.contains("://") {
        if !is_same_origin(BITBUCKET_API_BASE, trimmed) {
            return Err(ForgeError::UntrustedUrl(trimmed.to_owned()));
        }
        let path_ok = trimmed
            .strip_prefix("https://api.bitbucket.org/2.0")
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/') || rest.starts_with('?'));
        if !path_ok {
            return Err(ForgeError::UntrustedUrl(trimmed.to_owned()));
        }
        return Ok(trimmed.to_owned());
    }
    if trimmed.starts_with('/') {
        Ok(format!("{BITBUCKET_API_BASE}{trimmed}"))
    } else {
        Ok(format!("{BITBUCKET_API_BASE}/{trimmed}"))
    }
}

/// Normalizes a `Bitbucket` Cloud v2.0 pull request JSON object into [`NormalizedPullRequest`].
///
/// # Errors
///
/// Returns [`ForgeError::MalformedResponse`] if required fields (`id`, `title`, branch names) are absent.
pub fn normalize_bitbucket_pr_json(json: &Value) -> Result<NormalizedPullRequest, ForgeError> {
    let number = json_u64(json, "id")
        .ok_or_else(|| ForgeError::MalformedResponse("missing Bitbucket PR `id`".to_owned()))?;
    let title = json_str(json, "title")
        .ok_or_else(|| ForgeError::MalformedResponse("missing Bitbucket PR `title`".to_owned()))?
        .to_owned();
    let url = json
        .get("links")
        .and_then(|l| l.get("html"))
        .and_then(|h| json_str(h, "href"))
        .unwrap_or("")
        .to_owned();

    let state = match json_str(json, "state")
        .unwrap_or("OPEN")
        .to_ascii_uppercase()
        .as_str()
    {
        "MERGED" => PullRequestState::Merged,
        "DECLINED" | "SUPERSEDED" => PullRequestState::Closed,
        _ => PullRequestState::Open,
    };

    let head_branch = json
        .get("source")
        .and_then(|s| s.get("branch"))
        .and_then(|b| json_str(b, "name"))
        .ok_or_else(|| {
            ForgeError::MalformedResponse("missing Bitbucket `source.branch.name`".to_owned())
        })?
        .to_owned();
    let base_branch = json
        .get("destination")
        .and_then(|d| d.get("branch"))
        .and_then(|b| json_str(b, "name"))
        .ok_or_else(|| {
            ForgeError::MalformedResponse("missing Bitbucket `destination.branch.name`".to_owned())
        })?
        .to_owned();
    let head_sha = json
        .get("source")
        .and_then(|s| s.get("commit"))
        .and_then(|c| json_str(c, "hash"))
        .map(str::to_owned);

    Ok(NormalizedPullRequest {
        number,
        title,
        url,
        state,
        is_draft: false,
        head_branch,
        base_branch,
        head_sha,
        mergeability: PullRequestMergeability::Unknown,
        checks_state: PullRequestChecksState::Pending,
    })
}

fn validate_printable_secret<'a>(raw: &'a str, label: &str) -> Result<&'a str, ForgeError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || !trimmed.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
        return Err(ForgeError::InvalidCredentials(format!(
            "{label} must consist of non-empty printable ASCII without spaces or control chars"
        )));
    }
    Ok(trimmed)
}
