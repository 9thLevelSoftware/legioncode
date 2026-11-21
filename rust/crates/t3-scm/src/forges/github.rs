//! `GitHub` REST, GraphQL, `gh` CLI, and 10% quota-reserve policy adapter.

use http::{HeaderMap, Method, StatusCode};
use serde_json::{Value, json};
use t3_contracts::pull_requests::{PullRequestChecksState, PullRequestMergeability};

use crate::core::pr_helpers::{CheckEntry, RawCheckState, rollup_check_statuses};
use crate::core::rate_limit::QuotaSnapshot;
use crate::forges::{
    ForgeError, HttpRequestSpec, NormalizedPullRequest, json_str, json_u64,
    parse_standard_pr_state, validate_hostname,
};

/// Default per-host concurrent HTTP API request cap for `GitHub`.
pub const GITHUB_API_CONCURRENCY: usize = 8;

/// Default concurrent `gh` CLI process cap.
pub const GITHUB_CLI_CONCURRENCY: usize = 4;

/// Minimum quota reserve percentage kept off-limits to background requests (`10%`).
pub const GITHUB_QUOTA_RESERVE_PERCENT: u8 = 10;

/// Pinned `X-GitHub-Api-Version` header sent on REST requests.
pub const GITHUB_API_VERSION: &str = "2022-11-28";

/// `User-Agent` header sent to `GitHub` endpoints.
pub const GITHUB_USER_AGENT: &str = "t3code";

/// Resolved `GitHub` REST and GraphQL endpoint URLs for `github.com` or Enterprise Server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubEndpoint {
    /// Normalized lowercase hostname (`github.com` or GHE host).
    pub hostname: String,
    /// REST base URL (`https://api.github.com` or `https://<host>/api/v3`).
    pub api_base_url: String,
    /// GraphQL endpoint URL (`https://api.github.com/graphql` or `https://<host>/api/graphql`).
    pub graphql_url: String,
}

impl GitHubEndpoint {
    /// Resolves REST and GraphQL endpoints for `hostname`.
    ///
    /// # Errors
    ///
    /// Returns [`ForgeError::InvalidHost`] if `hostname` is malformed.
    ///
    /// ```
    /// use t3_scm::forges::github::GitHubEndpoint;
    ///
    /// let cloud = GitHubEndpoint::for_host("github.com")?;
    /// assert_eq!(cloud.api_base_url, "https://api.github.com");
    /// assert_eq!(cloud.graphql_url, "https://api.github.com/graphql");
    ///
    /// let ghe = GitHubEndpoint::for_host("git.corp.example")?;
    /// assert_eq!(ghe.api_base_url, "https://git.corp.example/api/v3");
    /// # Ok::<(), t3_scm::forges::ForgeError>(())
    /// ```
    pub fn for_host(hostname: &str) -> Result<Self, ForgeError> {
        let host = validate_hostname(hostname)?;
        if host == "github.com" || host == "api.github.com" {
            Ok(Self {
                hostname: "github.com".to_owned(),
                api_base_url: "https://api.github.com".to_owned(),
                graphql_url: "https://api.github.com/graphql".to_owned(),
            })
        } else {
            Ok(Self {
                api_base_url: format!("https://{host}/api/v3"),
                graphql_url: format!("https://{host}/api/graphql"),
                hostname: host,
            })
        }
    }
}

/// Classification of upstream `GitHub` REST or GraphQL rate-limit responses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitHubRateLimitSignal {
    /// Not rate-limited.
    Clear,
    /// Secondary or primary `429` / abuse limit / `RATE_LIMITED` GraphQL signal.
    RateLimited {
        /// Reason string from headers or error payload.
        reason: String,
    },
    /// Quota fell inside the protected 10% reserve for non-priority calls.
    QuotaReserveReached {
        /// Remaining calls reported.
        remaining: u64,
        /// Window limit reported.
        limit: u64,
    },
}

/// Validates bearer token header safety and builds a `GitHub` REST [`HttpRequestSpec`].
///
/// # Errors
///
/// Returns [`ForgeError::InvalidCredentials`] if `token` is empty or contains newlines.
pub fn build_github_rest_request(
    endpoint: &GitHubEndpoint,
    token: &str,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<HttpRequestSpec, ForgeError> {
    let trimmed_token = validate_token(token)?;
    let normalized_path = if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    };
    let headers = github_headers(
        trimmed_token,
        "application/vnd.github+json",
        body.is_some(),
        true,
    );
    Ok(HttpRequestSpec {
        method,
        url: format!("{}{normalized_path}", endpoint.api_base_url),
        headers,
        body: body.map(|val| val.to_string()),
    })
}

/// Builds a `GitHub` GraphQL POST [`HttpRequestSpec`].
///
/// # Errors
///
/// Returns [`ForgeError::InvalidCredentials`] if `token` is empty or contains newlines.
pub fn build_github_graphql_request(
    endpoint: &GitHubEndpoint,
    token: &str,
    query: &str,
    variables: Option<Value>,
) -> Result<HttpRequestSpec, ForgeError> {
    let trimmed_token = validate_token(token)?;
    let payload = match variables {
        Some(vars) => json!({ "query": query, "variables": vars }),
        None => json!({ "query": query }),
    };
    Ok(HttpRequestSpec {
        method: Method::POST,
        url: endpoint.graphql_url.clone(),
        headers: github_headers(trimmed_token, "application/json", true, false),
        body: Some(payload.to_string()),
    })
}

fn github_headers(
    token: &str,
    accept: &str,
    has_body: bool,
    include_api_version: bool,
) -> Vec<(String, String)> {
    let mut headers = vec![
        ("authorization".to_owned(), format!("Bearer {token}")),
        ("accept".to_owned(), accept.to_owned()),
        ("user-agent".to_owned(), GITHUB_USER_AGENT.to_owned()),
    ];
    if include_api_version {
        headers.push((
            "x-github-api-version".to_owned(),
            GITHUB_API_VERSION.to_owned(),
        ));
    }
    if has_body {
        headers.push(("content-type".to_owned(), "application/json".to_owned()));
    }
    headers
}

/// Returns isolated `gh` CLI environment overrides, gating `GH_ENTERPRISE_TOKEN` on Enterprise hosts.
#[must_use]
pub fn build_gh_cli_env(hostname: &str, token: Option<&str>) -> Vec<(String, String)> {
    let normalized = hostname.trim().to_ascii_lowercase();
    let mut env = vec![("GH_PROMPT_DISABLED".to_owned(), "1".to_owned())];
    if !normalized.is_empty() && normalized != "github.com" {
        env.push(("GH_HOST".to_owned(), normalized));
        if let Some(tok) = token
            && !tok.trim().is_empty()
        {
            env.push(("GH_ENTERPRISE_TOKEN".to_owned(), tok.trim().to_owned()));
        }
    } else if let Some(tok) = token
        && !tok.trim().is_empty()
    {
        env.push(("GH_TOKEN".to_owned(), tok.trim().to_owned()));
    }
    env
}

/// Enforces `GitHub`'s 10% quota reserve policy against a parsed [`QuotaSnapshot`].
#[must_use]
pub fn check_github_quota(snapshot: &QuotaSnapshot, allow_reserve: bool) -> GitHubRateLimitSignal {
    if snapshot.has_headroom(GITHUB_QUOTA_RESERVE_PERCENT, allow_reserve) {
        GitHubRateLimitSignal::Clear
    } else if snapshot.remaining == 0 {
        GitHubRateLimitSignal::RateLimited {
            reason: "GitHub primary rate limit exhausted (remaining=0)".to_owned(),
        }
    } else {
        GitHubRateLimitSignal::QuotaReserveReached {
            remaining: snapshot.remaining,
            limit: snapshot.limit,
        }
    }
}

/// Inspects an HTTP response status, headers, and optional JSON/text body for `GitHub` rate limits.
#[must_use]
pub fn classify_github_response(
    status: StatusCode,
    headers: &HeaderMap,
    body: &str,
) -> GitHubRateLimitSignal {
    if status == StatusCode::TOO_MANY_REQUESTS {
        return GitHubRateLimitSignal::RateLimited {
            reason: "HTTP 429 Too Many Requests".to_owned(),
        };
    }

    let remaining_zero = headers
        .get("x-ratelimit-remaining")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        == Some(0);
    let lower_body = body.to_ascii_lowercase();
    if status == StatusCode::FORBIDDEN
        && (remaining_zero
            || lower_body.contains("rate limit")
            || lower_body.contains("secondary rate")
            || lower_body.contains("abuse detection"))
    {
        return GitHubRateLimitSignal::RateLimited {
            reason: "HTTP 403 rate limit or abuse detection".to_owned(),
        };
    }

    if let Ok(parsed) = serde_json::from_str::<Value>(body)
        && let Some(errors) = parsed.get("errors").and_then(Value::as_array)
        && errors.iter().any(|err| {
            json_str(err, "type") == Some("RATE_LIMITED")
                || json_str(err, "message")
                    .is_some_and(|m| m.to_ascii_lowercase().contains("rate limit"))
        })
    {
        return GitHubRateLimitSignal::RateLimited {
            reason: "GraphQL RATE_LIMITED".to_owned(),
        };
    }

    GitHubRateLimitSignal::Clear
}

/// Normalizes a `GitHub` REST or GraphQL pull request JSON object into [`NormalizedPullRequest`].
///
/// # Errors
///
/// Returns [`ForgeError::MalformedResponse`] if required fields (`number`, `title`, branches) are missing.
pub fn normalize_github_pr_json(json: &Value) -> Result<NormalizedPullRequest, ForgeError> {
    let number = json_u64(json, "number")
        .ok_or_else(|| ForgeError::MalformedResponse("missing GitHub PR `number`".to_owned()))?;
    let title = json_str(json, "title")
        .ok_or_else(|| ForgeError::MalformedResponse("missing GitHub PR `title`".to_owned()))?
        .to_owned();
    let url = json_str(json, "html_url")
        .or_else(|| json_str(json, "url"))
        .unwrap_or("")
        .to_owned();

    let is_merged = json.get("merged").and_then(Value::as_bool) == Some(true)
        || json.get("merged_at").is_some_and(|v| !v.is_null())
        || json.get("mergedAt").is_some_and(|v| !v.is_null());
    let state = parse_standard_pr_state(json_str(json, "state").unwrap_or("open"), is_merged);
    let is_draft = json
        .get("draft")
        .and_then(Value::as_bool)
        .or_else(|| json.get("isDraft").and_then(Value::as_bool))
        .unwrap_or(false);

    let head_branch = json
        .get("head")
        .and_then(|h| json_str(h, "ref"))
        .or_else(|| json_str(json, "headRefName"))
        .ok_or_else(|| ForgeError::MalformedResponse("missing GitHub PR head branch".to_owned()))?
        .to_owned();
    let base_branch = json
        .get("base")
        .and_then(|b| json_str(b, "ref"))
        .or_else(|| json_str(json, "baseRefName"))
        .ok_or_else(|| ForgeError::MalformedResponse("missing GitHub PR base branch".to_owned()))?
        .to_owned();
    let head_sha = json
        .get("head")
        .and_then(|h| json_str(h, "sha"))
        .or_else(|| json_str(json, "headRefOid"))
        .map(str::to_owned);

    let mergeability = match json.get("mergeable").and_then(Value::as_bool) {
        Some(true) => PullRequestMergeability::Mergeable,
        Some(false) => PullRequestMergeability::Conflicting,
        None => match json_str(json, "mergeable") {
            Some("MERGEABLE") => PullRequestMergeability::Mergeable,
            Some("CONFLICTING") => PullRequestMergeability::Conflicting,
            _ => PullRequestMergeability::Unknown,
        },
    };

    let checks_state = extract_checks_state(json);

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
        checks_state,
    })
}

fn extract_checks_state(json: &Value) -> PullRequestChecksState {
    let Some(arr) = json.get("checks").and_then(Value::as_array) else {
        return PullRequestChecksState::Pending;
    };
    let entries: Vec<CheckEntry> = arr
        .iter()
        .map(|item| CheckEntry {
            name: json_str(item, "name").unwrap_or("check").to_owned(),
            context: None,
            state: RawCheckState::from_forge_str(
                json_str(item, "conclusion")
                    .or_else(|| json_str(item, "status"))
                    .unwrap_or("pending"),
            ),
            required: false,
            updated_at: None,
            details_url: None,
        })
        .collect();
    rollup_check_statuses(&entries)
}

fn validate_token(token: &str) -> Result<&str, ForgeError> {
    let trimmed = token.trim();
    if trimmed.is_empty() || trimmed.contains(['\r', '\n']) {
        return Err(ForgeError::InvalidCredentials(
            "GitHub token is empty or contains line breaks".to_owned(),
        ));
    }
    Ok(trimmed)
}
