//! `GitHub` GraphQL queries, aliased PR & review-comment batching (`988d677eb4`, `e187544d38`),
//! rate-limit signal classification, and pull-request normalization.

use std::fmt::Write as _;
use std::sync::OnceLock;

use http::{HeaderMap, Method, StatusCode};
use regex::Regex;
use serde_json::{Value, json};
use t3_contracts::pull_requests::{PullRequestChecksState, PullRequestMergeability};

use crate::core::pr_helpers::{CheckEntry, RawCheckState, rollup_check_statuses};
use crate::core::rate_limit::QuotaSnapshot;
use crate::forges::github::{
    GITHUB_QUOTA_RESERVE_PERCENT, GitHubEndpoint, github_headers, validate_token,
};
use crate::forges::{
    ForgeError, HttpRequestSpec, NormalizedPullRequest, json_str, json_u64, parse_standard_pr_state,
};

/// Maximum number of aliased GraphQL nodes packed into one request (`STAT_ALIASES_PER_REQUEST`,
/// `GitHubPullRequestApi.ts:476`).
pub const GITHUB_GRAPHQL_ALIAS_BATCH_SIZE: usize = 25;

fn github_slug_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let compiled = Regex::new(r"^[A-Za-z0-9._-]+$");
        // INFALLIBLE: constant GitHub owner/repository identifier validator.
        compiled.expect("github_slug_regex must compile")
    })
}

fn validate_slug<'a>(value: &'a str, label: &str) -> Result<&'a str, ForgeError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || !github_slug_regex().is_match(trimmed) {
        return Err(ForgeError::MalformedResponse(format!(
            "invalid GitHub {label} `{trimmed}` for GraphQL query"
        )));
    }
    Ok(trimmed)
}

fn validate_node_id(node_id: &str) -> Result<&str, ForgeError> {
    let trimmed = node_id.trim();
    if trimmed.is_empty() || trimmed.contains(['"', '\\', '\r', '\n']) {
        return Err(ForgeError::MalformedResponse(
            "invalid GitHub GraphQL node id".to_owned(),
        ));
    }
    Ok(trimmed)
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

/// Target PR identifier packed into an aliased [`build_aliased_pr_summary_batch`] GraphQL query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrLookupItem {
    /// Repository owner login or organization.
    pub owner: String,
    /// Repository name slug.
    pub name: String,
    /// Pull request number.
    pub number: u64,
}

/// Unfinished review thread needing a batched follow-up page of comments (`988d677eb4`, `e187544d38`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewCommentBatchTarget {
    /// GitHub GraphQL node ID of the `PullRequestReviewThread`.
    pub thread_node_id: String,
    /// GraphQL `after` cursor returned by the initial `comments(first: 10)` page.
    pub after_cursor: String,
}

/// Builds a `GitHub` GraphQL POST [`HttpRequestSpec`].
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

/// Packs `items` into aliased GraphQL PR summary requests of at most
/// [`GITHUB_GRAPHQL_ALIAS_BATCH_SIZE`] (`25`) aliases each (`988d677eb4`).
pub fn build_aliased_pr_summary_batch(
    endpoint: &GitHubEndpoint,
    token: &str,
    items: &[PrLookupItem],
) -> Result<Vec<HttpRequestSpec>, ForgeError> {
    let mut specs = Vec::new();
    for chunk in items.chunks(GITHUB_GRAPHQL_ALIAS_BATCH_SIZE) {
        let mut query = String::from("query AliasedPullRequestSummaries {\n");
        for (idx, item) in chunk.iter().enumerate() {
            let owner = validate_slug(&item.owner, "owner")?;
            let name = validate_slug(&item.name, "repository")?;
            let _ = writeln!(
                query,
                "  pr{idx}: repository(owner: \"{owner}\", name: \"{name}\") {{\n    pullRequest(number: {}) {{\n      number\n      title\n      url\n      state\n      isDraft\n      mergedAt\n      headRefName\n      baseRefName\n      headRefOid\n      mergeable\n      comments {{ totalCount }}\n      reviewThreads {{ totalCount }}\n    }}\n  }}",
                item.number
            );
        }
        query.push('}');
        specs.push(build_github_graphql_request(endpoint, token, &query, None)?);
    }
    Ok(specs)
}

/// Decodes aliased `pr0..pr{count-1}` results from [`build_aliased_pr_summary_batch`] response JSON.
pub fn parse_aliased_pr_summary_batch(
    body_json: &Value,
    expected_count: usize,
) -> Result<Vec<Option<NormalizedPullRequest>>, ForgeError> {
    let Some(data) = body_json.get("data").and_then(Value::as_object) else {
        return Err(ForgeError::MalformedResponse(
            "missing `data` object in GitHub GraphQL batch response".to_owned(),
        ));
    };
    let mut out = Vec::with_capacity(expected_count);
    for idx in 0..expected_count {
        let alias = format!("pr{idx}");
        let pr = data
            .get(&alias)
            .and_then(|repo| repo.get("pullRequest"))
            .filter(|v| v.is_object());
        match pr {
            Some(node) => out.push(Some(normalize_github_pr_json(node)?)),
            None => out.push(None),
        }
    }
    Ok(out)
}

/// Builds a bounded `ReviewThreads` GraphQL query fetching `reviewThreads(first: 100)` with
/// `comments(first: 10)` per thread to stay under GitHub's GraphQL resource ceiling (`988d677eb4`, `e187544d38`).
pub fn build_review_threads_page_request(
    endpoint: &GitHubEndpoint,
    token: &str,
    owner: &str,
    name: &str,
    number: u64,
    after_cursor: Option<&str>,
) -> Result<HttpRequestSpec, ForgeError> {
    let owner = validate_slug(owner, "owner")?;
    let name = validate_slug(name, "repository")?;
    let query = r"
query ReviewThreadsPage($owner: String!, $name: String!, $number: Int!, $after: String) {
  viewer { login }
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      reviewThreads(first: 100, after: $after) {
        totalCount
        pageInfo { hasNextPage endCursor }
        nodes {
          id
          isResolved
          isOutdated
          path
          line
          comments(first: 10) {
            totalCount
            pageInfo { hasNextPage endCursor }
            nodes { id body createdAt lastEditedAt url isMinimized author { __typename login } }
          }
        }
      }
      latestReviews(first: 50) {
        nodes { state isMinimized author { __typename login } }
      }
    }
  }
}
";
    let vars = json!({
        "owner": owner,
        "name": name,
        "number": number,
        "after": after_cursor,
    });
    build_github_graphql_request(endpoint, token, query.trim(), Some(vars))
}

/// Packs follow-up review-thread comment cursor fetches into aliased GraphQL node queries
/// (`GITHUB_GRAPHQL_ALIAS_BATCH_SIZE` = `25`, `988d677eb4`, `e187544d38`).
pub fn build_review_thread_comments_batch(
    endpoint: &GitHubEndpoint,
    token: &str,
    targets: &[ReviewCommentBatchTarget],
) -> Result<Vec<HttpRequestSpec>, ForgeError> {
    let mut specs = Vec::new();
    for chunk in targets.chunks(GITHUB_GRAPHQL_ALIAS_BATCH_SIZE) {
        let mut query = String::from("query ReviewThreadCommentsBatch {\n");
        for (idx, target) in chunk.iter().enumerate() {
            let id = validate_node_id(&target.thread_node_id)?;
            let cursor = validate_node_id(&target.after_cursor)?;
            let _ = writeln!(
                query,
                "  th{idx}: node(id: \"{id}\") {{\n    ... on PullRequestReviewThread {{\n      comments(first: 100, after: \"{cursor}\") {{\n        pageInfo {{ hasNextPage endCursor }}\n        nodes {{ id body createdAt lastEditedAt url isMinimized author {{ __typename login }} }}\n      }}\n    }}\n  }}"
            );
        }
        query.push('}');
        specs.push(build_github_graphql_request(endpoint, token, &query, None)?);
    }
    Ok(specs)
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

/// Normalizes a `GitHub` REST or GraphQL PR JSON object into [`NormalizedPullRequest`].
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
