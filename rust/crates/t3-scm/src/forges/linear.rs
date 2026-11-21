//! `Linear` GraphQL client spec (raw `Authorization: <api_key>` without `Bearer ` prefix).

use std::sync::OnceLock;

use http::{HeaderMap, Method};
use regex::Regex;
use serde_json::{Value, json};

use crate::forges::{ForgeError, HttpRequestSpec, json_str};

/// Canonical `Linear` GraphQL API endpoint URL.
pub const LINEAR_GRAPHQL_URL: &str = "https://api.linear.app/graphql";

/// Authentication modes accepted by `Linear`'s GraphQL API.
///
/// Personal API keys MUST be passed verbatim in `Authorization: <api_key>` without `"Bearer "`,
/// whereas OAuth access tokens use `"Bearer <token>"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinearAuth {
    /// Personal `Linear` API key emitted raw as `Authorization: <api_key>` (NO `Bearer ` prefix).
    ApiKey(String),
    /// OAuth access token emitted as `Authorization: Bearer <token>`.
    OAuthBearer(String),
}

/// Rate-limit headers parsed from a `Linear` GraphQL HTTP response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LinearRateLimitHeaders {
    /// Remaining request count in current window (`x-ratelimit-requests-remaining`).
    pub requests_remaining: Option<u64>,
    /// Epoch milliseconds until request quota resets (`x-ratelimit-requests-reset`).
    pub requests_reset_ms: Option<u64>,
    /// Remaining GraphQL complexity budget (`x-ratelimit-complexity-remaining`).
    pub complexity_remaining: Option<u64>,
    /// Epoch milliseconds until complexity budget resets (`x-ratelimit-complexity-reset`).
    pub complexity_reset_ms: Option<u64>,
}

/// Typed `Linear` GraphQL `extensions.code` error classifications.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinearErrorCode {
    /// Upstream rate-limit (`RATELIMITED`).
    RateLimited,
    /// Invalid/revoked credential (`AUTHENTICATION_ERROR`).
    AuthenticationError,
    /// Any other GraphQL error code or message.
    Other(String),
}

/// Reference extracted from a `Linear` issue URL or `TEAM-123` identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinearIssueRef {
    /// Team key uppercase prefix (e.g. `"ENG"`).
    pub team_key: String,
    /// Numeric issue sequence (e.g. `123`).
    pub number: u64,
    /// Canonical `TEAM-123` identifier string.
    pub identifier: String,
}

fn linear_identifier_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let compiled = Regex::new(r"(?i)^([a-z][a-z0-9]{0,15})-(\d+)$");
        // INFALLIBLE: static Linear identifier regex (`TEAM-123`) verified at compile/test time.
        compiled.expect("valid Linear identifier regex")
    })
}

/// Builds a `Linear` GraphQL HTTP POST request using [`LinearAuth::ApiKey`]
/// (emitting raw `Authorization: <api_key>` WITHOUT a `Bearer ` prefix).
///
/// # Errors
///
/// Returns [`ForgeError::InvalidCredentials`] if `api_key` is empty or contains line breaks.
///
/// ```
/// use t3_scm::forges::linear::{LINEAR_GRAPHQL_URL, build_linear_api_key_request};
///
/// let req = build_linear_api_key_request("lin_api_secret123", "{ viewer { id } }", None)?;
/// assert_eq!(req.url, LINEAR_GRAPHQL_URL);
/// assert!(req.headers.contains(&("authorization".into(), "lin_api_secret123".into())));
/// assert!(!req.headers.iter().any(|(_, v)| v.starts_with("Bearer ")));
/// # Ok::<(), t3_scm::forges::ForgeError>(())
/// ```
pub fn build_linear_api_key_request(
    api_key: &str,
    query: &str,
    variables: Option<Value>,
) -> Result<HttpRequestSpec, ForgeError> {
    build_linear_graphql_request(&LinearAuth::ApiKey(api_key.to_owned()), query, variables)
}

/// Builds a `Linear` GraphQL HTTP POST [`HttpRequestSpec`].
///
/// For [`LinearAuth::ApiKey`], emits `Authorization: <api_key>` directly without `Bearer `.
///
/// # Errors
///
/// Returns [`ForgeError::InvalidCredentials`] if credentials are empty or contain CR/LF,
/// or [`ForgeError::MalformedResponse`] if `query` is blank.
pub fn build_linear_graphql_request(
    auth: &LinearAuth,
    query: &str,
    variables: Option<Value>,
) -> Result<HttpRequestSpec, ForgeError> {
    if query.trim().is_empty() {
        return Err(ForgeError::MalformedResponse(
            "Linear GraphQL query must not be empty".to_owned(),
        ));
    }
    let auth_header_value = match auth {
        LinearAuth::ApiKey(raw_key) => validate_linear_secret(raw_key)?.to_owned(),
        LinearAuth::OAuthBearer(token) => format!("Bearer {}", validate_linear_secret(token)?),
    };
    let payload = match variables {
        Some(vars) => json!({ "query": query, "variables": vars }),
        None => json!({ "query": query }),
    };
    Ok(HttpRequestSpec {
        method: Method::POST,
        url: LINEAR_GRAPHQL_URL.to_owned(),
        headers: vec![
            ("authorization".to_owned(), auth_header_value),
            ("content-type".to_owned(), "application/json".to_owned()),
            ("accept".to_owned(), "application/json".to_owned()),
        ],
        body: Some(payload.to_string()),
    })
}

/// Parses `Linear`'s request and complexity rate-limit response headers (millisecond reset epochs).
#[must_use]
pub fn parse_linear_rate_limit_headers(headers: &HeaderMap) -> LinearRateLimitHeaders {
    LinearRateLimitHeaders {
        requests_remaining: header_u64(headers, "x-ratelimit-requests-remaining"),
        requests_reset_ms: header_u64(headers, "x-ratelimit-requests-reset"),
        complexity_remaining: header_u64(headers, "x-ratelimit-complexity-remaining"),
        complexity_reset_ms: header_u64(headers, "x-ratelimit-complexity-reset"),
    }
}

/// Extracts `Linear` GraphQL `extensions.code` error classifications from a response body.
#[must_use]
pub fn classify_linear_graphql_errors(body: &Value) -> Option<LinearErrorCode> {
    let errors = body.get("errors").and_then(Value::as_array)?;
    for err in errors {
        let code = err
            .get("extensions")
            .and_then(|ext| json_str(ext, "code"))
            .unwrap_or("");
        match code {
            "RATELIMITED" => return Some(LinearErrorCode::RateLimited),
            "AUTHENTICATION_ERROR" => return Some(LinearErrorCode::AuthenticationError),
            other if !other.is_empty() => return Some(LinearErrorCode::Other(other.to_owned())),
            _ => {
                if let Some(msg) = json_str(err, "message") {
                    return Some(LinearErrorCode::Other(msg.to_owned()));
                }
            }
        }
    }
    None
}

/// Parses a `TEAM-123` `Linear` identifier into a normalized [`LinearIssueRef`].
#[must_use]
pub fn parse_linear_identifier(raw: &str) -> Option<LinearIssueRef> {
    let caps = linear_identifier_regex().captures(raw.trim())?;
    let team_key = caps.get(1)?.as_str().to_ascii_uppercase();
    let number = caps.get(2)?.as_str().parse::<u64>().ok()?;
    let identifier = format!("{team_key}-{number}");
    Some(LinearIssueRef {
        team_key,
        number,
        identifier,
    })
}

/// Extracts a [`LinearIssueRef`] from a `https://linear.app/<workspace>/issue/<TEAM-123>/...` URL.
#[must_use]
pub fn parse_linear_issue_url(url: &str) -> Option<LinearIssueRef> {
    let trimmed = url.trim().split(['?', '#']).next()?;
    let rest = trimmed.strip_prefix("https://linear.app/")?;
    let mut segs = rest.split('/');
    let _workspace = segs.next()?;
    if segs.next()? != "issue" {
        return None;
    }
    parse_linear_identifier(segs.next()?)
}

/// Decoded `Linear` issue record returned by `GraphQL` issue queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinearIssueDetails {
    /// Internal `Linear` UUID.
    pub id: String,
    /// Canonical `TEAM-123` key.
    pub identifier: String,
    /// Issue summary title.
    pub title: String,
    /// Workflow state name, if present.
    pub state_name: Option<String>,
    /// Canonical web URL, if present.
    pub url: Option<String>,
}

/// Extracts [`LinearIssueDetails`] from a `Linear` GraphQL JSON response body.
pub fn parse_linear_issue_details(body_json: &Value) -> Result<LinearIssueDetails, ForgeError> {
    let issue = body_json
        .get("data")
        .and_then(|d| d.get("issue"))
        .filter(|v| v.is_object())
        .ok_or_else(|| {
            ForgeError::MalformedResponse(
                "missing `data.issue` object in Linear response".to_owned(),
            )
        })?;
    let id = json_str(issue, "id")
        .ok_or_else(|| ForgeError::MalformedResponse("missing Linear issue `id`".to_owned()))?
        .to_owned();
    let identifier = json_str(issue, "identifier")
        .ok_or_else(|| {
            ForgeError::MalformedResponse("missing Linear issue `identifier`".to_owned())
        })?
        .to_owned();
    let title = json_str(issue, "title")
        .ok_or_else(|| ForgeError::MalformedResponse("missing Linear issue `title`".to_owned()))?
        .to_owned();
    let state_name = issue
        .get("state")
        .and_then(|s| json_str(s, "name"))
        .map(str::to_owned);
    let url = json_str(issue, "url").map(str::to_owned);
    Ok(LinearIssueDetails {
        id,
        identifier,
        title,
        state_name,
        url,
    })
}

fn validate_linear_secret(raw: &str) -> Result<&str, ForgeError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.contains(['\r', '\n']) {
        return Err(ForgeError::InvalidCredentials(
            "Linear credential is empty or contains line breaks".to_owned(),
        ));
    }
    Ok(trimmed)
}

fn header_u64(headers: &HeaderMap, key: &str) -> Option<u64> {
    headers.get(key)?.to_str().ok()?.trim().parse::<u64>().ok()
}
