//! `GitHub` REST, GHE host detection, `gh` CLI / `git credential fill` auth, and quota helpers.

use std::sync::OnceLock;

use http::Method;
use regex::Regex;
use serde_json::Value;

use crate::core::discovery::parse_git_remote_url;
pub use crate::forges::github_graphql::{
    GITHUB_GRAPHQL_ALIAS_BATCH_SIZE, GitHubRateLimitSignal, PrLookupItem, ReviewCommentBatchTarget,
    build_aliased_pr_summary_batch, build_github_graphql_request,
    build_review_thread_comments_batch, build_review_threads_page_request, check_github_quota,
    classify_github_response, normalize_github_pr_json, parse_aliased_pr_summary_batch,
};
use crate::forges::{CliCommandSpec, ForgeError, HttpRequestSpec, validate_hostname};

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

fn github_host_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let compiled = Regex::new(
            r"^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]*[a-z0-9])?)*(?::\d{1,5})?$",
        );
        // INFALLIBLE: constant hostname validation pattern (`fd475bd722`).
        compiled.expect("github_host_regex must compile")
    })
}

/// Validates that `host` is a normalized bare hostname without scheme, userinfo, or path (`fd475bd722`).
#[must_use]
pub fn is_github_host(host: &str) -> bool {
    github_host_regex().is_match(&host.trim().to_ascii_lowercase())
}

/// Returns `true` when `host` uses `GitHub` Cloud endpoints (`github.com` or `*.ghe.com` cloud tenants).
#[must_use]
pub fn is_github_dot_com_or_ghe_cloud(host: &str) -> bool {
    let lower = host.trim().to_ascii_lowercase();
    lower == "github.com" || lower == "api.github.com" || lower.ends_with(".ghe.com")
}

/// Resolved `GitHub` REST and GraphQL endpoint URLs for `github.com`, `*.ghe.com`, or GHE Server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubEndpoint {
    /// Normalized lowercase hostname (`github.com`, `<tenant>.ghe.com`, or GHE Server host).
    pub hostname: String,
    /// REST base URL (`https://api.github.com`, `https://api.<tenant>.ghe.com`, or `https://<host>/api/v3`).
    pub api_base_url: String,
    /// GraphQL endpoint URL (`https://api.github.com/graphql`, `https://api.<tenant>.ghe.com/graphql`, or `https://<host>/api/graphql`).
    pub graphql_url: String,
}

impl GitHubEndpoint {
    /// Returns `true` when this endpoint targets a `*.ghe.com` cloud tenant or GHE Server host.
    #[must_use]
    pub fn is_enterprise(&self) -> bool {
        self.hostname != "github.com" && self.hostname != "api.github.com"
    }

    /// Resolves REST and GraphQL endpoints for `hostname` (`fd475bd722`).
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
    /// let ghe_cloud = GitHubEndpoint::for_host("acme.ghe.com")?;
    /// assert_eq!(ghe_cloud.api_base_url, "https://api.acme.ghe.com");
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
        } else if host.ends_with(".ghe.com") {
            Ok(Self {
                api_base_url: format!("https://api.{host}"),
                graphql_url: format!("https://api.{host}/graphql"),
                hostname: host,
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

/// Detects the target [`GitHubEndpoint`] directly from an HTTPS or SSH git remote URL (`fd475bd722`).
pub fn detect_github_endpoint_from_remote(remote_url: &str) -> Result<GitHubEndpoint, ForgeError> {
    let parsed = parse_git_remote_url(remote_url).map_err(|err| ForgeError::InvalidHost {
        host: remote_url.to_owned(),
        reason: err.to_string(),
    })?;
    GitHubEndpoint::for_host(&parsed.host)
}

/// Validates bearer token header safety and builds a `GitHub` REST [`HttpRequestSpec`].
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

pub(crate) fn github_headers(
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

/// Resolves an environment variable GitHub token, scoping `GH_ENTERPRISE_TOKEN` to `GH_HOST`
/// so enterprise credentials cannot leak to arbitrary hosts.
#[must_use]
pub fn resolve_github_env_token<F>(hostname: &str, get_env: F) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    let host = hostname.trim().to_ascii_lowercase();
    if !is_github_host(&host) {
        return None;
    }
    let candidates: &[&str] = if is_github_dot_com_or_ghe_cloud(&host) {
        &["GH_TOKEN", "GITHUB_TOKEN"]
    } else if get_env("GH_HOST")
        .map(|v| v.trim().to_ascii_lowercase())
        .as_deref()
        == Some(host.as_str())
    {
        &["GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"]
    } else {
        &[]
    };
    candidates.iter().find_map(|name| {
        get_env(name)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    })
}

/// Returns isolated `gh` CLI environment overrides, gating `GH_ENTERPRISE_TOKEN` on Enterprise hosts.
#[must_use]
pub fn build_gh_cli_env(hostname: &str, token: Option<&str>) -> Vec<(String, String)> {
    let normalized = hostname.trim().to_ascii_lowercase();
    let mut env = vec![
        ("GH_PROMPT_DISABLED".to_owned(), "1".to_owned()),
        ("GH_DEBUG".to_owned(), String::new()),
    ];
    if !normalized.is_empty() && normalized != "github.com" {
        env.push(("GH_HOST".to_owned(), normalized.clone()));
        if let Some(tok) = token
            && !tok.trim().is_empty()
        {
            let var = if normalized.ends_with(".ghe.com") {
                "GH_TOKEN"
            } else {
                "GH_ENTERPRISE_TOKEN"
            };
            env.push((var.to_owned(), tok.trim().to_owned()));
        }
    } else if let Some(tok) = token
        && !tok.trim().is_empty()
    {
        env.push(("GH_TOKEN".to_owned(), tok.trim().to_owned()));
    }
    env
}

/// Parsed output from `git credential fill` (`550cb3363c`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCredentialFillResult {
    /// Credential protocol (typically `"https"`).
    pub protocol: Option<String>,
    /// Matched host reported by git credential helper.
    pub host: Option<String>,
    /// Username if supplied by helper.
    pub username: Option<String>,
    /// Secret password / personal access token / OAuth token.
    pub password: String,
}

/// Builds a non-interactive `git credential fill` [`CliCommandSpec`] for `host` (`550cb3363c`).
///
/// Rejects line breaks, `NUL`, or invalid hostnames to prevent git credential attribute injection.
pub fn build_git_credential_fill_spec(
    protocol: &str,
    host: &str,
    path: Option<&str>,
) -> Result<CliCommandSpec, ForgeError> {
    let clean_proto = protocol.trim().to_ascii_lowercase();
    if !matches!(clean_proto.as_str(), "https" | "http") {
        return Err(ForgeError::InvalidHost {
            host: protocol.to_owned(),
            reason: "git credential protocol must be `http` or `https`".to_owned(),
        });
    }
    let clean_host = validate_hostname(host)?;
    let mut stdin = format!("protocol={clean_proto}\nhost={clean_host}\n");
    if let Some(p) = path.map(str::trim).filter(|p| !p.is_empty()) {
        if p.contains(['\r', '\n', '\0']) {
            return Err(ForgeError::InvalidCredentials(
                "git credential path contains forbidden control characters".to_owned(),
            ));
        }
        stdin.push_str("path=");
        stdin.push_str(p.trim_start_matches('/'));
        stdin.push('\n');
    }
    stdin.push('\n');
    Ok(CliCommandSpec {
        program: "git".to_owned(),
        args: vec!["credential".to_owned(), "fill".to_owned()],
        env: vec![("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned())],
        stdin: Some(stdin),
    })
}

/// Parses `key=value` output emitted by `git credential fill` (`550cb3363c`).
///
/// Stops at the blank terminator line and requires a non-empty `password`.
pub fn parse_git_credential_fill_output(
    stdout: &str,
) -> Result<GitCredentialFillResult, ForgeError> {
    let mut protocol = None;
    let mut host = None;
    let mut username = None;
    let mut password = None;

    for raw_line in stdout.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.is_empty() {
            break;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let val = value.trim();
        match key.trim() {
            "protocol" if !val.is_empty() => protocol = Some(val.to_owned()),
            "host" if !val.is_empty() => host = Some(val.to_owned()),
            "username" if !val.is_empty() => username = Some(val.to_owned()),
            "password" if !val.is_empty() => password = Some(val.to_owned()),
            _ => {}
        }
    }

    let password = password.ok_or_else(|| {
        ForgeError::AuthRequired("git credential fill returned no password for host".to_owned())
    })?;
    Ok(GitCredentialFillResult {
        protocol,
        host,
        username,
        password,
    })
}

pub(crate) fn validate_token(token: &str) -> Result<&str, ForgeError> {
    let trimmed = token.trim();
    if trimmed.is_empty() || trimmed.contains(['\r', '\n']) {
        return Err(ForgeError::InvalidCredentials(
            "GitHub token is empty or contains line breaks".to_owned(),
        ));
    }
    Ok(trimmed)
}
