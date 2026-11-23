//! Axum SCM & issue-tracker routes (`/api/scm/pr`, `/api/scm/diff`) backed by `LiveForgeClient`.

use std::path::Path;
use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use legion_contracts::permissions::EnvironmentScope;
use legion_scm::checkpoints::diff_query::{ComputedDiff, DiffQueryOptions};
use legion_scm::core::discovery::ParsedRemote;
use legion_scm::forges::NormalizedPullRequest;
use legion_scm::forges::bitbucket::BitbucketAuth;
use legion_scm::forges::github::GitHubEndpoint;
use legion_scm::forges::linear::LinearIssueDetails;
use legion_scm::live_client::{
    CreatePullRequestParams, ForgeClientTarget, LiveForgeClient, MergeMethod,
    MergePullRequestOutcome, MergePullRequestParams,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::ServerRuntimeError;
use crate::http_api::RuntimeHttpState;
use crate::http_auth_guard::{authorize_request, authorize_scopes, parse_authorized_json};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScmPrRequest {
    action: Option<String>,
    provider: Option<String>,
    hostname: Option<String>,
    api_base_url: Option<String>,
    graphql_url: Option<String>,
    linear_endpoint: Option<String>,
    token: Option<String>,
    user: Option<String>,
    app_password: Option<String>,
    repo: Option<String>,
    number: Option<u64>,
    state: Option<String>,
    issue: Option<String>,
    title: Option<String>,
    body: Option<String>,
    head: Option<String>,
    base: Option<String>,
    draft: Option<bool>,
    commit_title: Option<String>,
    commit_message: Option<String>,
    method: Option<String>,
    strip_agent_credits: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScmDiffRequest {
    repo_dir: String,
    base_ref: Option<String>,
    context_lines: Option<usize>,
    ignore_whitespace: Option<bool>,
    max_output_bytes: Option<usize>,
}

/// `GET /api/scm/pr` — fetch/list a PR or Linear issue (`OrchestrationRead`).
pub(crate) async fn handle_get_scm_pr(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    Query(req): Query<ScmPrRequest>,
) -> Result<Response, ServerRuntimeError> {
    let _principal = authorize_request(
        &state,
        &headers,
        &[EnvironmentScope::OrchestrationRead],
        false,
    )?;
    execute_scm_pr_request(&req).await
}

/// `POST /api/scm/pr` — fetch/list/create/merge PRs or fetch Linear issues.
pub(crate) async fn handle_post_scm_pr(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ServerRuntimeError> {
    let (principal, req): (_, ScmPrRequest) = parse_authorized_json(
        &state,
        &headers,
        &[EnvironmentScope::OrchestrationRead],
        false,
        &body,
    )?;
    let action = req.action.as_deref().map_or("fetch", str::trim);
    if matches!(action, "create" | "merge") {
        authorize_scopes(&principal, &[EnvironmentScope::SourceControlWrite], true)?;
    }
    execute_scm_pr_request(&req).await
}

/// `GET /api/scm/diff` — compute live workspace diff & remote discovery (`OrchestrationRead`).
pub(crate) async fn handle_get_scm_diff(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    Query(req): Query<ScmDiffRequest>,
) -> Result<Response, ServerRuntimeError> {
    let _principal = authorize_request(
        &state,
        &headers,
        &[EnvironmentScope::OrchestrationRead],
        false,
    )?;
    execute_scm_diff_request(&req).await
}

/// `POST /api/scm/diff` — compute live workspace diff & remote discovery (`OrchestrationRead`).
pub(crate) async fn handle_post_scm_diff(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ServerRuntimeError> {
    let (_principal, req): (_, ScmDiffRequest) = parse_authorized_json(
        &state,
        &headers,
        &[EnvironmentScope::OrchestrationRead],
        false,
        &body,
    )?;
    execute_scm_diff_request(&req).await
}

async fn execute_scm_pr_request(req: &ScmPrRequest) -> Result<Response, ServerRuntimeError> {
    let action = req.action.as_deref().map_or("fetch", str::trim);
    let mut client = LiveForgeClient::new()?;
    if let Some(linear_url) = req.linear_endpoint.as_deref().filter(|s| !s.is_empty()) {
        client = client.with_linear_endpoint(linear_url);
    }

    if action == "linear" || req.provider.as_deref() == Some("linear") {
        let token = req.token.as_deref().ok_or_else(|| {
            ServerRuntimeError::BadRequest("Linear request requires `token`".to_owned())
        })?;
        let issue_id = req.issue.as_deref().ok_or_else(|| {
            ServerRuntimeError::BadRequest("Linear request requires `issue`".to_owned())
        })?;
        let details = client.fetch_linear_issue(token, issue_id).await?;
        return Ok((
            StatusCode::OK,
            Json(json!({ "issue": linear_to_json(&details) })),
        )
            .into_response());
    }

    let target = build_forge_target(req)?;
    let repo = req
        .repo
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| ServerRuntimeError::BadRequest("SCM request requires `repo`".to_owned()))?;

    match action {
        "fetch" => {
            let number = req.number.ok_or_else(|| {
                ServerRuntimeError::BadRequest("`fetch` action requires `number`".to_owned())
            })?;
            let pr = client.fetch_pull_request(&target, repo, number).await?;
            Ok((
                StatusCode::OK,
                Json(json!({ "pullRequest": pr_to_json(&pr) })),
            )
                .into_response())
        }
        "list" => {
            let prs = client
                .list_pull_requests(&target, repo, req.state.as_deref())
                .await?;
            let encoded: Vec<Value> = prs.iter().map(pr_to_json).collect();
            Ok((StatusCode::OK, Json(json!({ "pullRequests": encoded }))).into_response())
        }
        "create" => {
            let params = CreatePullRequestParams {
                repo: repo.to_owned(),
                title: require_non_empty(req.title.as_deref(), "title")?,
                body: req.body.clone().unwrap_or_default(),
                head: require_non_empty(req.head.as_deref(), "head")?,
                base: require_non_empty(req.base.as_deref(), "base")?,
                draft: req.draft.unwrap_or(false),
            };
            let pr = client.create_pull_request(&target, &params).await?;
            Ok((
                StatusCode::CREATED,
                Json(json!({ "pullRequest": pr_to_json(&pr) })),
            )
                .into_response())
        }
        "merge" => {
            let number = req.number.ok_or_else(|| {
                ServerRuntimeError::BadRequest("`merge` action requires `number`".to_owned())
            })?;
            let method = parse_merge_method(req.method.as_deref())?;
            let outcome = client
                .merge_pull_request(
                    &target,
                    &MergePullRequestParams {
                        repo: repo.to_owned(),
                        number,
                        commit_title: req.commit_title.clone(),
                        commit_message: req.commit_message.clone(),
                        method,
                        strip_agent_credits: req.strip_agent_credits.unwrap_or(true),
                    },
                )
                .await?;
            Ok((StatusCode::OK, Json(merge_to_json(&outcome))).into_response())
        }
        other => Err(ServerRuntimeError::BadRequest(format!(
            "unsupported `/api/scm/pr` action `{other}`"
        ))),
    }
}

async fn execute_scm_diff_request(req: &ScmDiffRequest) -> Result<Response, ServerRuntimeError> {
    let client = LiveForgeClient::new()?;
    let repo_dir = Path::new(&req.repo_dir);
    let base_ref = req
        .base_ref
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("HEAD");
    let defaults = DiffQueryOptions::default();
    let opts = DiffQueryOptions {
        context_lines: req.context_lines.unwrap_or(defaults.context_lines),
        ignore_whitespace: req.ignore_whitespace.unwrap_or(defaults.ignore_whitespace),
        max_output_bytes: req.max_output_bytes.unwrap_or(defaults.max_output_bytes),
    };
    let diff = client
        .compute_live_git_diff(repo_dir, base_ref, &opts)
        .await?;
    let remote = client.discover_workspace_remote(repo_dir).await.ok();
    Ok((StatusCode::OK, Json(diff_to_json(&diff, remote.as_ref()))).into_response())
}

fn build_forge_target(req: &ScmPrRequest) -> Result<ForgeClientTarget, ServerRuntimeError> {
    let provider = req.provider.as_deref().unwrap_or("github");
    match provider {
        "github" => {
            let token = require_non_empty(req.token.as_deref(), "token")?;
            let hostname = req.hostname.as_deref().unwrap_or("github.com");
            let mut endpoint = GitHubEndpoint::for_host(hostname).map_err(|err| {
                ServerRuntimeError::BadRequest(format!("invalid GitHub host `{hostname}`: {err}"))
            })?;
            if let Some(base) = req.api_base_url.as_deref().filter(|s| !s.is_empty()) {
                base.trim_end_matches('/')
                    .clone_into(&mut endpoint.api_base_url);
            }
            if let Some(gql) = req.graphql_url.as_deref().filter(|s| !s.is_empty()) {
                gql.clone_into(&mut endpoint.graphql_url);
            }
            Ok(ForgeClientTarget::GitHub { endpoint, token })
        }
        "bitbucket" => {
            let auth = if let (Some(email), Some(api_token)) = (
                req.user.as_deref().filter(|s| !s.is_empty()),
                req.app_password.as_deref().filter(|s| !s.is_empty()),
            ) {
                BitbucketAuth::Basic {
                    email: email.to_owned(),
                    api_token: api_token.to_owned(),
                }
            } else {
                BitbucketAuth::Bearer(require_non_empty(req.token.as_deref(), "token")?)
            };
            Ok(ForgeClientTarget::Bitbucket { auth })
        }
        "forgejo" => Ok(ForgeClientTarget::Forgejo {
            hostname: require_non_empty(req.hostname.as_deref(), "hostname")?,
            token: require_non_empty(req.token.as_deref(), "token")?,
        }),
        other => Err(ServerRuntimeError::BadRequest(format!(
            "unsupported SCM provider `{other}`"
        ))),
    }
}

fn parse_merge_method(raw: Option<&str>) -> Result<MergeMethod, ServerRuntimeError> {
    match raw.map_or("merge", str::trim) {
        "merge" => Ok(MergeMethod::Merge),
        "squash" => Ok(MergeMethod::Squash),
        "rebase" => Ok(MergeMethod::Rebase),
        other => Err(ServerRuntimeError::BadRequest(format!(
            "unsupported merge method `{other}`"
        ))),
    }
}

fn require_non_empty(val: Option<&str>, field: &str) -> Result<String, ServerRuntimeError> {
    val.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| ServerRuntimeError::BadRequest(format!("missing required `{field}`")))
}

fn pr_to_json(pr: &NormalizedPullRequest) -> Value {
    json!({
        "number": pr.number,
        "title": pr.title,
        "url": pr.url,
        "state": format!("{:?}", pr.state).to_lowercase(),
        "isDraft": pr.is_draft,
        "headBranch": pr.head_branch,
        "baseBranch": pr.base_branch,
        "headSha": pr.head_sha,
        "mergeability": format!("{:?}", pr.mergeability),
        "checksState": format!("{:?}", pr.checks_state),
    })
}

fn merge_to_json(outcome: &MergePullRequestOutcome) -> Value {
    json!({ "merged": outcome.merged, "sha": outcome.sha, "message": outcome.message })
}

fn linear_to_json(issue: &LinearIssueDetails) -> Value {
    json!({
        "id": issue.id, "identifier": issue.identifier,
        "title": issue.title, "stateName": issue.state_name, "url": issue.url,
    })
}

fn diff_to_json(diff: &ComputedDiff, remote: Option<&ParsedRemote>) -> Value {
    json!({
        "diff": diff.diff, "truncated": diff.truncated,
        "filesChanged": diff.files_changed, "insertions": diff.insertions,
        "deletions": diff.deletions,
        "remote": remote.map(|r| json!({
            "raw": r.raw, "host": r.host, "port": r.port,
            "owner": r.owner, "repo": r.repo, "fullName": r.full_name,
        })),
    })
}
