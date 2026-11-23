//! High-level async [`LiveForgeClient`] bridging forge specs, rate limiting, `Linear`, and `git`.

use std::path::Path;

use serde_json::{Value, json};

use crate::checkpoints::diff_query::{ComputedDiff, DiffQueryOptions};
use crate::core::discovery::ParsedRemote;
use crate::core::rate_limit::SourceControlRateLimit;
pub use crate::forges::linear::LinearIssueDetails;
use crate::forges::linear::{
    LinearErrorCode, build_linear_api_key_request, classify_linear_graphql_errors,
    parse_linear_issue_details,
};
use crate::forges::{CliCommandSpec, ForgeError, HttpRequestSpec, NormalizedPullRequest};
pub use crate::live_forge_ops::{
    CreatePullRequestParams, ForgeClientTarget, MergeMethod, MergePullRequestOutcome,
    MergePullRequestParams,
};
use crate::live_forge_ops::{
    build_create_pr_spec, build_fetch_pr_spec, build_list_prs_spec, build_merge_pr_spec,
    ensure_success_and_parse, normalize_pr_for_target, validate_repo_slug,
};
use crate::live_git_cli::{
    LiveCliOutput, collect_safe_base_env, compute_live_git_diff, discover_workspace_remote,
    execute_cli_spec,
};
use crate::live_http::{LiveClientError, LiveHttpResponse, build_http_client, execute_http_spec};

/// Async live execution adapter for HTTP forge requests, CLI commands, and real `git` operations.
///
/// ```no_run
/// # async fn demo() -> Result<(), Box<dyn std::error::Error>> {
/// use legion_scm::live_client::LiveForgeClient;
///
/// let client = LiveForgeClient::new()?;
/// let issue = client.fetch_linear_issue("lin_api_key", "ENG-123").await?;
/// assert_eq!(issue.identifier, "ENG-123");
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct LiveForgeClient {
    http: reqwest::Client,
    rate_limit: SourceControlRateLimit,
    base_env: Vec<(String, String)>,
    allow_reserve: bool,
    linear_endpoint_url: Option<String>,
}

impl LiveForgeClient {
    /// Creates a [`LiveForgeClient`] using default rate-limit settings and sanitized process env.
    pub fn new() -> Result<Self, LiveClientError> {
        Self::with_rate_limit(SourceControlRateLimit::default())
    }

    /// Creates a [`LiveForgeClient`] backed by an explicit [`SourceControlRateLimit`] tracker.
    pub fn with_rate_limit(rate_limit: SourceControlRateLimit) -> Result<Self, LiveClientError> {
        Ok(Self {
            http: build_http_client()?,
            rate_limit,
            base_env: collect_safe_base_env(),
            allow_reserve: false,
            linear_endpoint_url: None,
        })
    }

    /// Configures whether HTTP calls may consume the protected 10% quota reserve.
    #[must_use]
    pub const fn with_allow_reserve(mut self, allow: bool) -> Self {
        self.allow_reserve = allow;
        self
    }

    /// Replaces the baseline environment passed after `env_clear()` to CLI subprocesses.
    #[must_use]
    pub fn with_base_env(mut self, base_env: Vec<(String, String)>) -> Self {
        self.base_env = base_env;
        self
    }

    /// Overrides the `Linear` GraphQL URL (used when routing against local mock servers).
    #[must_use]
    pub fn with_linear_endpoint(mut self, url: impl Into<String>) -> Self {
        self.linear_endpoint_url = Some(url.into());
        self
    }

    /// Exposes the underlying per-host [`SourceControlRateLimit`] state.
    #[must_use]
    pub const fn rate_limit(&self) -> &SourceControlRateLimit {
        &self.rate_limit
    }

    /// Executes an [`HttpRequestSpec`], checking per-host limits, same-origin redirects,
    /// and updating quota/backoff state from upstream headers.
    pub async fn execute_http(
        &self,
        spec: &HttpRequestSpec,
    ) -> Result<LiveHttpResponse, LiveClientError> {
        execute_http_spec(&self.http, &self.rate_limit, spec, self.allow_reserve).await
    }

    /// Executes a [`CliCommandSpec`] with `env_clear()`, explicit env vars, and piped `stdin`.
    pub async fn execute_cli(
        &self,
        spec: &CliCommandSpec,
    ) -> Result<LiveCliOutput, LiveClientError> {
        execute_cli_spec(spec, &self.base_env, None).await
    }

    /// Executes a [`CliCommandSpec`] scoped to `working_dir`.
    pub async fn execute_cli_in_dir(
        &self,
        spec: &CliCommandSpec,
        working_dir: &Path,
    ) -> Result<LiveCliOutput, LiveClientError> {
        execute_cli_spec(spec, &self.base_env, Some(working_dir)).await
    }

    /// Discovers and parses the git `origin` remote of `repo_dir`.
    pub async fn discover_workspace_remote(
        &self,
        repo_dir: &Path,
    ) -> Result<ParsedRemote, LiveClientError> {
        discover_workspace_remote(repo_dir, None, &self.base_env).await
    }

    /// Computes a real unified diff inside `repo_dir` against `base_ref`.
    pub async fn compute_live_git_diff(
        &self,
        repo_dir: &Path,
        base_ref: &str,
        opts: &DiffQueryOptions,
    ) -> Result<ComputedDiff, LiveClientError> {
        compute_live_git_diff(repo_dir, base_ref, opts, &self.base_env).await
    }

    /// Fetches a single pull request from the configured forge.
    pub async fn fetch_pull_request(
        &self,
        target: &ForgeClientTarget,
        repo: &str,
        number: u64,
    ) -> Result<NormalizedPullRequest, LiveClientError> {
        let slug = validate_repo_slug(repo)?;
        let spec = build_fetch_pr_spec(target, slug, number)?;
        let resp = self.execute_http(&spec).await?;
        ensure_success_and_parse(target, &resp, |val| normalize_pr_for_target(target, val))
    }

    /// Lists pull requests from the configured forge filtered by optional state (`open`/`closed`/`all`).
    pub async fn list_pull_requests(
        &self,
        target: &ForgeClientTarget,
        repo: &str,
        state: Option<&str>,
    ) -> Result<Vec<NormalizedPullRequest>, LiveClientError> {
        let slug = validate_repo_slug(repo)?;
        let spec = build_list_prs_spec(target, slug, state)?;
        let resp = self.execute_http(&spec).await?;
        ensure_success_and_parse(target, &resp, |val| {
            let items = val
                .as_array()
                .or_else(|| val.get("values").and_then(Value::as_array))
                .ok_or_else(|| {
                    ForgeError::MalformedResponse("expected PR array payload".to_owned())
                })?;
            items
                .iter()
                .map(|item| normalize_pr_for_target(target, item))
                .collect::<Result<Vec<_>, _>>()
        })
    }

    /// Creates a pull request on the configured forge.
    pub async fn create_pull_request(
        &self,
        target: &ForgeClientTarget,
        input: &CreatePullRequestParams,
    ) -> Result<NormalizedPullRequest, LiveClientError> {
        let slug = validate_repo_slug(&input.repo)?;
        let spec = build_create_pr_spec(target, slug, input)?;
        let resp = self.execute_http(&spec).await?;
        ensure_success_and_parse(target, &resp, |val| normalize_pr_for_target(target, val))
    }

    /// Merges a pull request after optionally stripping agent credit trailers from commit messages.
    pub async fn merge_pull_request(
        &self,
        target: &ForgeClientTarget,
        input: &MergePullRequestParams,
    ) -> Result<MergePullRequestOutcome, LiveClientError> {
        let slug = validate_repo_slug(&input.repo)?;
        let spec = build_merge_pr_spec(target, slug, input)?;
        let resp = self.execute_http(&spec).await?;
        ensure_success_and_parse(target, &resp, |val| {
            Ok(MergePullRequestOutcome {
                merged: val.get("merged").and_then(Value::as_bool).unwrap_or(true),
                sha: val.get("sha").and_then(Value::as_str).map(str::to_owned),
                message: val
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("merged")
                    .to_owned(),
            })
        })
    }

    /// Queries a `Linear` issue via GraphQL using raw `Authorization: <api_key>` (no `Bearer `).
    pub async fn fetch_linear_issue(
        &self,
        api_key: &str,
        identifier: &str,
    ) -> Result<LinearIssueDetails, LiveClientError> {
        let query = "query IssueById($id: String!) { issue(id: $id) { id identifier title url state { name } } }";
        let mut spec =
            build_linear_api_key_request(api_key, query, Some(json!({ "id": identifier })))?;
        if let Some(override_url) = &self.linear_endpoint_url {
            spec.url = override_url.clone();
        }
        let resp = self.execute_http(&spec).await?;
        if !resp.status.is_success() {
            return Err(LiveClientError::HttpStatus {
                status: resp.status.as_u16(),
                url: resp.final_url,
                body: resp.body,
            });
        }
        let body_json = resp.json()?;
        if let Some(code) = classify_linear_graphql_errors(&body_json) {
            return Err(match code {
                LinearErrorCode::RateLimited => {
                    ForgeError::RateLimited("Linear RATELIMITED".to_owned())
                }
                LinearErrorCode::AuthenticationError => {
                    ForgeError::AuthRequired("Linear AUTHENTICATION_ERROR".to_owned())
                }
                LinearErrorCode::Other(msg) => ForgeError::MalformedResponse(msg),
            }
            .into());
        }
        Ok(parse_linear_issue_details(&body_json)?)
    }
}
