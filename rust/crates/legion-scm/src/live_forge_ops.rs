//! Target request specs, PR create/merge inputs, and Linear/forge response parsers for `live_client`.

use http::Method;
use serde_json::{Value, json};

use crate::core::pr_helpers::{MergeMessageInput, build_merge_commit_message};
use crate::forges::bitbucket::{
    BitbucketAuth, build_bitbucket_request, normalize_bitbucket_pr_json,
};
use crate::forges::forgejo::{build_forgejo_http_request, normalize_forgejo_pr_json};
use crate::forges::github::{
    GitHubEndpoint, GitHubRateLimitSignal, build_github_rest_request, classify_github_response,
    normalize_github_pr_json,
};
use crate::forges::{ForgeError, HttpRequestSpec, NormalizedPullRequest};
use crate::live_http::{LiveClientError, LiveHttpResponse};

/// Target HTTP forge backend and credentials used by [`crate::live_client::LiveForgeClient`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForgeClientTarget {
    /// `GitHub` Cloud or Enterprise endpoint (also accepts local mock base URLs).
    GitHub {
        /// Resolved REST & GraphQL endpoints.
        endpoint: GitHubEndpoint,
        /// `GitHub` personal access or OAuth bearer token.
        token: String,
    },
    /// `Bitbucket` Cloud v2.0 REST backend.
    Bitbucket {
        /// Bearer or Basic credential bundle.
        auth: BitbucketAuth,
    },
    /// `Forgejo` / `Gitea` `/api/v1` host.
    Forgejo {
        /// Normalized hostname (`codeberg.org` or self-hosted).
        hostname: String,
        /// `Forgejo` API token.
        token: String,
    },
}

/// Parameters required to open a pull request on a forge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatePullRequestParams {
    /// Repository slug (`owner/repo`).
    pub repo: String,
    /// Pull request title.
    pub title: String,
    /// Pull request description body.
    pub body: String,
    /// Source branch name.
    pub head: String,
    /// Base/target branch name.
    pub base: String,
    /// Whether to open as a draft pull request.
    pub draft: bool,
}

/// Supported pull request merge strategies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeMethod {
    /// Standard merge commit (`merge`).
    Merge,
    /// Squash and merge (`squash`).
    Squash,
    /// Rebase and merge (`rebase`).
    Rebase,
}

impl MergeMethod {
    pub(crate) const fn as_github_str(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Squash => "squash",
            Self::Rebase => "rebase",
        }
    }
}

/// Parameters for merging an existing pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergePullRequestParams {
    /// Repository slug (`owner/repo`).
    pub repo: String,
    /// Pull request number.
    pub number: u64,
    /// Optional commit title override.
    pub commit_title: Option<String>,
    /// Optional commit message body.
    pub commit_message: Option<String>,
    /// Merge strategy.
    pub method: MergeMethod,
    /// When `true`, strip automated AI agent attribution trailers from commit messages.
    pub strip_agent_credits: bool,
}

/// Result returned when merging a pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergePullRequestOutcome {
    /// Whether the merge completed on upstream.
    pub merged: bool,
    /// Resulting merge commit `SHA`, if reported.
    pub sha: Option<String>,
    /// Server response status message.
    pub message: String,
}

pub(crate) fn validate_repo_slug(repo: &str) -> Result<&str, LiveClientError> {
    let trimmed = repo.trim().trim_matches('/');
    let Some((owner, name)) = trimmed.split_once('/') else {
        return Err(LiveClientError::InvalidInput(format!(
            "repository slug `{repo}` must be `<owner>/<repo>`"
        )));
    };
    if owner.is_empty()
        || name.is_empty()
        || trimmed.contains("..")
        || trimmed.contains(['?', '#', ' '])
    {
        return Err(LiveClientError::InvalidInput(format!(
            "unsafe repository slug `{repo}`"
        )));
    }
    Ok(trimmed)
}

pub(crate) fn build_fetch_pr_spec(
    target: &ForgeClientTarget,
    slug: &str,
    number: u64,
) -> Result<HttpRequestSpec, ForgeError> {
    match target {
        ForgeClientTarget::GitHub { endpoint, token } => build_github_rest_request(
            endpoint,
            token,
            Method::GET,
            &format!("/repos/{slug}/pulls/{number}"),
            None,
        ),
        ForgeClientTarget::Bitbucket { auth } => build_bitbucket_request(
            auth,
            Method::GET,
            &format!("/repositories/{slug}/pullrequests/{number}"),
            None,
        ),
        ForgeClientTarget::Forgejo { hostname, token } => build_forgejo_http_request(
            hostname,
            token,
            Method::GET,
            &format!("/repos/{slug}/pulls/{number}"),
            None,
        ),
    }
}

pub(crate) fn build_list_prs_spec(
    target: &ForgeClientTarget,
    slug: &str,
    state: Option<&str>,
) -> Result<HttpRequestSpec, ForgeError> {
    let query = state.map_or(String::new(), |s| format!("?state={s}"));
    match target {
        ForgeClientTarget::GitHub { endpoint, token } => build_github_rest_request(
            endpoint,
            token,
            Method::GET,
            &format!("/repos/{slug}/pulls{query}"),
            None,
        ),
        ForgeClientTarget::Bitbucket { auth } => build_bitbucket_request(
            auth,
            Method::GET,
            &format!("/repositories/{slug}/pullrequests{query}"),
            None,
        ),
        ForgeClientTarget::Forgejo { hostname, token } => build_forgejo_http_request(
            hostname,
            token,
            Method::GET,
            &format!("/repos/{slug}/pulls{query}"),
            None,
        ),
    }
}

pub(crate) fn build_create_pr_spec(
    target: &ForgeClientTarget,
    slug: &str,
    input: &CreatePullRequestParams,
) -> Result<HttpRequestSpec, ForgeError> {
    match target {
        ForgeClientTarget::GitHub { endpoint, token } => build_github_rest_request(
            endpoint,
            token,
            Method::POST,
            &format!("/repos/{slug}/pulls"),
            Some(json!({
                "title": input.title,
                "body": input.body,
                "head": input.head,
                "base": input.base,
                "draft": input.draft,
            })),
        ),
        ForgeClientTarget::Bitbucket { auth } => build_bitbucket_request(
            auth,
            Method::POST,
            &format!("/repositories/{slug}/pullrequests"),
            Some(json!({
                "title": input.title,
                "description": input.body,
                "source": { "branch": { "name": input.head } },
                "destination": { "branch": { "name": input.base } },
            })),
        ),
        ForgeClientTarget::Forgejo { hostname, token } => build_forgejo_http_request(
            hostname,
            token,
            Method::POST,
            &format!("/repos/{slug}/pulls"),
            Some(json!({
                "title": input.title,
                "body": input.body,
                "head": input.head,
                "base": input.base,
            })),
        ),
    }
}

pub(crate) fn build_merge_pr_spec(
    target: &ForgeClientTarget,
    slug: &str,
    input: &MergePullRequestParams,
) -> Result<HttpRequestSpec, ForgeError> {
    let msg = build_merge_commit_message(&MergeMessageInput {
        title: input
            .commit_title
            .clone()
            .unwrap_or_else(|| format!("Merge pull request #{}", input.number)),
        number: Some(input.number),
        body: input.commit_message.clone(),
        include_body: input.commit_message.is_some(),
        strip_agent_credits: input.strip_agent_credits,
        co_authors: Vec::new(),
    });
    match target {
        ForgeClientTarget::GitHub { endpoint, token } => build_github_rest_request(
            endpoint,
            token,
            Method::PUT,
            &format!("/repos/{slug}/pulls/{}/merge", input.number),
            Some(json!({
                "commit_title": msg.subject,
                "commit_message": msg.body.unwrap_or_default(),
                "merge_method": input.method.as_github_str(),
            })),
        ),
        ForgeClientTarget::Bitbucket { auth } => build_bitbucket_request(
            auth,
            Method::POST,
            &format!("/repositories/{slug}/pullrequests/{}/merge", input.number),
            Some(json!({ "message": msg.full_message })),
        ),
        ForgeClientTarget::Forgejo { hostname, token } => build_forgejo_http_request(
            hostname,
            token,
            Method::POST,
            &format!("/repos/{slug}/pulls/{}/merge", input.number),
            Some(json!({
                "Do": input.method.as_github_str(),
                "merge_message_field": msg.full_message,
            })),
        ),
    }
}

pub(crate) fn normalize_pr_for_target(
    target: &ForgeClientTarget,
    json: &Value,
) -> Result<NormalizedPullRequest, ForgeError> {
    match target {
        ForgeClientTarget::GitHub { .. } => normalize_github_pr_json(json),
        ForgeClientTarget::Bitbucket { .. } => normalize_bitbucket_pr_json(json),
        ForgeClientTarget::Forgejo { .. } => normalize_forgejo_pr_json(json),
    }
}

pub(crate) fn ensure_success_and_parse<T, F>(
    target: &ForgeClientTarget,
    resp: &LiveHttpResponse,
    parse: F,
) -> Result<T, LiveClientError>
where
    F: FnOnce(&Value) -> Result<T, ForgeError>,
{
    if matches!(target, ForgeClientTarget::GitHub { .. })
        && let GitHubRateLimitSignal::RateLimited { reason } =
            classify_github_response(resp.status, &resp.headers, &resp.body)
    {
        return Err(ForgeError::RateLimited(reason).into());
    }
    if !resp.status.is_success() {
        return Err(LiveClientError::HttpStatus {
            status: resp.status.as_u16(),
            url: resp.final_url.clone(),
            body: resp.body.clone(),
        });
    }
    let val = resp.json()?;
    Ok(parse(&val)?)
}
