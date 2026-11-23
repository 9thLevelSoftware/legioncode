//! Lightweight `GitHub` link metadata queries (`#40595`), reusable repository issue bindings
//! (`#40592`, `#39159`), Activity cross-reference parsing (`#40768`), and self-hosted `Forgejo`
//! issue links (`#40271`).

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{Value, json};

use crate::forges::github::GitHubEndpoint;
use crate::forges::github_graphql::build_github_graphql_request;
use crate::forges::{ForgeError, HttpRequestSpec, json_str, json_u64};

/// Minimal `GitHub` GraphQL query fetching only link metadata (`number`, `title`, `url`) when
/// attaching an issue to a thread (`#40595`).
pub const GITHUB_ISSUE_LINK_METADATA_QUERY: &str = "query IssueLinkMetadata($owner: String!, $name: String!, $number: Int!) { repository(owner: $owner, name: $name) { issue(number: $number) { number title url } } }";

/// Minimal issue link fields returned by `IssueLinkMetadata` (`#40595`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubIssueLinkMetadata {
    /// Formatted `#123` display key.
    pub display_key: String,
    /// Issue title.
    pub title: String,
    /// Canonical web URL.
    pub url: String,
}

/// Builds a `GitHub` `IssueLinkMetadata` GraphQL request (`#40595`).
///
/// # Errors
///
/// Returns [`ForgeError`] if `owner`, `name`, or `token` is invalid.
pub fn build_github_issue_link_metadata_request(
    endpoint: &GitHubEndpoint,
    token: &str,
    owner: &str,
    name: &str,
    number: u64,
) -> Result<HttpRequestSpec, ForgeError> {
    let vars = json!({
        "owner": owner.trim(),
        "name": name.trim(),
        "number": number,
    });
    build_github_graphql_request(
        endpoint,
        token,
        GITHUB_ISSUE_LINK_METADATA_QUERY,
        Some(vars),
    )
}

/// Parses `data.repository.issue.{number, title, url}` from `IssueLinkMetadata` (`#40595`).
/// Returns `Ok(None)` when the issue does not exist (`issue: null`).
///
/// # Errors
///
/// Returns [`ForgeError::MalformedResponse`] when `data` is missing or an issue object omits `number`/`title`/`url`.
pub fn parse_github_issue_link_metadata(
    body_json: &Value,
) -> Result<Option<GitHubIssueLinkMetadata>, ForgeError> {
    let Some(data) = body_json.get("data").and_then(Value::as_object) else {
        return Err(ForgeError::MalformedResponse(
            "missing `data` object in GitHub IssueLinkMetadata response".to_owned(),
        ));
    };
    let Some(issue) = data
        .get("repository")
        .and_then(|r| r.get("issue"))
        .filter(|v| !v.is_null())
    else {
        return Ok(None);
    };
    let number = json_u64(issue, "number").ok_or_else(|| {
        ForgeError::MalformedResponse("IssueLinkMetadata issue missing `number`".to_owned())
    })?;
    let title = json_str(issue, "title")
        .ok_or_else(|| {
            ForgeError::MalformedResponse("IssueLinkMetadata issue missing `title`".to_owned())
        })?
        .to_owned();
    let url = json_str(issue, "url")
        .ok_or_else(|| {
            ForgeError::MalformedResponse("IssueLinkMetadata issue missing `url`".to_owned())
        })?
        .to_owned();
    Ok(Some(GitHubIssueLinkMetadata {
        display_key: format!("#{number}"),
        title,
        url,
    }))
}

/// Normalized host + lowercased scope binding resolved from a repository identity (`#39159`, `#40592`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryIssueBinding {
    /// Canonical lowercase host (`"github.com"`, `"gitlab.com"`, etc.).
    pub host: String,
    /// Lowercased issue scope (`"owner/repo"`).
    pub scope: String,
    /// Optional upstream fork alias scope.
    pub scope_alias: Option<String>,
}

/// Resolves issue tracker bindings for a repository and optional upstream fork (`#39159`),
/// putting upstream first when distinct and deduplicating matching `(host, scope)`.
#[must_use]
pub fn resolve_git_host_issue_bindings(
    host: &str,
    primary_scope: &str,
    upstream_scope: Option<&str>,
) -> Vec<RepositoryIssueBinding> {
    let norm_host = host.trim().to_ascii_lowercase();
    let norm_primary = primary_scope.trim().to_ascii_lowercase();
    if norm_host.is_empty() || norm_primary.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    if let Some(up) = upstream_scope
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty() && s != &norm_primary)
    {
        out.push(RepositoryIssueBinding {
            host: norm_host.clone(),
            scope: up,
            scope_alias: Some(norm_primary.clone()),
        });
    }
    out.push(RepositoryIssueBinding {
        host: norm_host,
        scope: norm_primary,
        scope_alias: None,
    });
    out
}

/// Cache reusing repository issue bindings across repeated markdown link classifications (`#40592`).
#[derive(Debug, Default)]
pub struct RepositoryIssueBindingCache {
    by_identity_key: HashMap<String, Vec<RepositoryIssueBinding>>,
}

impl RepositoryIssueBindingCache {
    /// Creates an empty [`RepositoryIssueBindingCache`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns cached bindings for `identity_key`, computing once when absent (`#40592`).
    pub fn get_or_compute(
        &mut self,
        identity_key: &str,
        host: &str,
        primary_scope: &str,
        upstream_scope: Option<&str>,
    ) -> &[RepositoryIssueBinding] {
        if !self.by_identity_key.contains_key(identity_key) {
            let computed = resolve_git_host_issue_bindings(host, primary_scope, upstream_scope);
            self.by_identity_key
                .insert(identity_key.to_owned(), computed);
        }
        &self.by_identity_key[identity_key]
    }
}

/// Target kind referenced by an issue Activity cross-reference (`#40768`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueReferenceKind {
    /// Another issue.
    Issue,
    /// A pull/merge request.
    PullRequest,
}

/// Clickable target metadata carried by a `referenced` issue activity event (`#40768`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueEventReference {
    /// Canonical `owner/repo` (or `GitLab` group/project path).
    pub repository: String,
    /// Target issue or PR/MR number.
    pub number: u64,
    /// Whether the reference points to an issue or pull request.
    pub kind: IssueReferenceKind,
    /// Web URL opened when activating the Activity entry.
    pub url: String,
}

/// Extracts `(detail, Option<IssueEventReference>)` from a `GitHub` `CrossReferencedEvent.source`
/// JSON object (`#40768`).
#[must_use]
pub fn parse_github_cross_referenced_source(
    source: &Value,
) -> Option<(String, Option<IssueEventReference>)> {
    let typename = json_str(source, "__typename")?;
    let kind = match typename {
        "PullRequest" => IssueReferenceKind::PullRequest,
        "Issue" => IssueReferenceKind::Issue,
        _ => return None,
    };
    let repository = source
        .get("repository")
        .and_then(|r| json_str(r, "nameWithOwner"))
        .map(str::trim)
        .filter(|s| !s.is_empty())?
        .to_owned();
    let number = json_u64(source, "number").filter(|&n| n > 0)?;
    let detail = format!("{repository}#{number}");
    let reference = json_str(source, "url")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|url| IssueEventReference {
            repository,
            number,
            kind,
            url: url.to_owned(),
        });
    Some((detail, reference))
}

fn gitlab_mention_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let compiled = Regex::new(r"^(?:merge request ([\w.\-/]*)!|issue ([\w.\-/]*)#)([1-9]\d*)$");
        // INFALLIBLE: linear GitLab mention system-note matcher (`#40768`).
        compiled.expect("gitlab_mention_regex must compile")
    })
}

/// Parses a `GitLab` `"mentioned in merge request !12"` note (with or without the leading
/// `"mentioned in "` prefix) into a clickable [`IssueEventReference`] resolved relative to
/// `issue_url`'s instance/subpath (`#40768`).
#[must_use]
pub fn parse_gitlab_mention_reference(
    detail: &str,
    issue_url: &str,
    scope: &str,
) -> Option<IssueEventReference> {
    let trimmed = detail.trim();
    let normalized = trimmed
        .strip_prefix("mentioned in ")
        .map_or(trimmed, str::trim_start);
    let caps = gitlab_mention_regex().captures(normalized)?;
    let project_url = issue_url.split("/-/").next()?;
    let suffix = format!("/{}", scope.to_ascii_lowercase());
    if !project_url.to_ascii_lowercase().ends_with(&suffix) {
        return None;
    }
    let mr_path = caps.get(1).map(|m| m.as_str());
    let issue_path = caps.get(2).map(|m| m.as_str());
    let is_mr = mr_path.is_some();
    let path = mr_path.or(issue_path).unwrap_or("");
    let number = caps.get(3)?.as_str().parse::<u64>().ok()?;

    let repository = if path.is_empty() {
        scope.to_owned()
    } else if path.contains('/') {
        path.to_owned()
    } else {
        let ns_prefix = match scope.rfind('/') {
            Some(idx) => &scope[..=idx],
            None => "",
        };
        format!("{ns_prefix}{path}")
    };
    let base = &project_url[..project_url.len() - scope.len()];
    let segment = if is_mr { "merge_requests" } else { "issues" };
    Some(IssueEventReference {
        url: format!("{base}{repository}/-/{segment}/{number}"),
        repository,
        number,
        kind: if is_mr {
            IssueReferenceKind::PullRequest
        } else {
            IssueReferenceKind::Issue
        },
    })
}

/// Returns `true` when `hostname` identifies a public or recognizable `Forgejo`/`Gitea` host (`#40271`).
#[must_use]
pub fn is_forgejo_host(hostname: &str) -> bool {
    let lower = hostname.trim().to_ascii_lowercase();
    lower == "codeberg.org"
        || lower
            .split('.')
            .any(|seg| seg == "forgejo" || seg == "gitea")
}

/// Parsed `Forgejo`/`Gitea` issue URL (`<scope>`, `<number>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgejoIssueLink {
    /// `<owner>/<repo>` scope.
    pub scope: String,
    /// Numeric issue identifier.
    pub number: u64,
}

/// Resolves a `Forgejo` `/<owner>/<repo>/issues/<n>` URL when either `known_forgejo_host` is true
/// (a project on that self-hosted hostname already vouches for `Forgejo`) or `is_forgejo_host` holds (`#40271`).
#[must_use]
pub fn parse_forgejo_issue_url(url: &str, known_forgejo_host: bool) -> Option<ForgejoIssueLink> {
    let rest = url
        .trim()
        .strip_prefix("https://")
        .or_else(|| url.trim().strip_prefix("http://"))?;
    let clean = rest.split(['?', '#']).next()?;
    let (authority, path_part) = clean.split_once('/')?;
    let host_only = authority.rsplit('@').next().unwrap_or(authority);
    let hostname = host_only.split(':').next().unwrap_or(host_only);
    if !(known_forgejo_host || is_forgejo_host(hostname)) {
        return None;
    }
    let segments: Vec<&str> = path_part
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    if segments.len() == 4 && segments[2] == "issues" {
        let number = segments[3].parse::<u64>().ok().filter(|&n| n > 0)?;
        return Some(ForgejoIssueLink {
            scope: format!("{}/{}", segments[0], segments[1]),
            number,
        });
    }
    None
}
