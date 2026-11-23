//! Issue and pull-request forge trait abstractions for durable workflows.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::traits::{BoxFuture, IssueAddress, PullRequestAddress, WorkflowForgeError};

/// Issue query filter for `issues.list` and `issues.opened`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueQuery {
    /// Project ID or workspace path.
    pub project: String,
    /// Issue state filter (`open`, `closed`, `all`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// Required labels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    /// Free-text search query.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Assignee filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    /// Sort field (`updated` or `created`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<String>,
    /// Max results.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Pull request query filter for `pullRequests.list` and `pullRequests.opened`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestQuery {
    /// Project ID or workspace path.
    pub project: String,
    /// State filter (`open`, `closed`, `merged`, `all`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// Free-text search query.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Max results.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Input for `issues.create`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueCreateInput {
    /// Project ID or workspace path.
    pub project: String,
    /// Issue title.
    pub title: String,
    /// Issue body markdown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Labels to attach.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    /// Assignees to attach.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignees: Option<Vec<String>>,
}

/// Individual CI check summary on a pull request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestCheck {
    /// Check status (`success`, `failure`, `cancelled`, `action-required`, `pending`, ...).
    pub status: String,
    /// Whether this check is marked required by branch protection.
    #[serde(default)]
    pub required: bool,
    /// Whether this is a placeholder check entry that should be ignored.
    #[serde(default)]
    pub placeholder: bool,
}

/// Pull request detail returned by `pullRequests.read` and watch polling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestDetail {
    /// Project ID.
    pub project_id: String,
    /// Normalized host (`github`, `gitlab`, `forgejo`, ...).
    pub host: String,
    /// Normalized `owner/repo` slug.
    pub repository: String,
    /// PR number.
    pub number: i64,
    /// PR title.
    pub title: String,
    /// PR web URL.
    pub url: String,
    /// PR state (`open`, `closed`, `merged`).
    pub state: String,
    /// Mergeability (`mergeable`, `conflicting`, `unknown`).
    pub mergeability: String,
    /// Checks reported for the head commit.
    #[serde(default)]
    pub checks: Vec<PullRequestCheck>,
}

/// Condition for `pullRequests.wait`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PullRequestUntil {
    /// Wait until all required (or listed) checks finish.
    Checks,
    /// Wait until PR is merged or closed.
    Merged,
    /// Wait until PR is closed or merged.
    Closed,
}

/// Persisted watch descriptor inside `workflow_waits.watch_json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestWatch {
    /// Always `"pullRequest"`.
    pub kind: String,
    /// Target settlement condition.
    pub until: PullRequestUntil,
    /// Pull request address.
    pub pull_request: PullRequestAddress,
}

/// Condensed pull-request standing returned when a PR wait resolves (`PullRequestStanding`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestStanding {
    /// `open`, `closed`, or `merged`.
    pub state: String,
    /// `passing`, `failing`, `pending`, or `none`.
    pub checks: String,
    /// `mergeable`, `conflicting`, or `unknown`.
    pub mergeability: String,
    /// Pull request web URL.
    pub url: String,
}

/// Checks settle after 2 minutes when the host lists no checks (`NO_CHECKS_AFTER_MS`).
pub const NO_CHECKS_AFTER_MS: i64 = 120_000;

/// Computes [`PullRequestStanding`] from a [`PullRequestDetail`] (`standingOf`).
pub fn standing_of(detail: &PullRequestDetail) -> PullRequestStanding {
    let listed: Vec<&PullRequestCheck> = detail.checks.iter().filter(|c| !c.placeholder).collect();
    let has_required = listed.iter().any(|c| c.required);
    let counted: Vec<&PullRequestCheck> = if has_required {
        listed.into_iter().filter(|c| c.required).collect()
    } else {
        listed
    };
    let failing = counted.iter().any(|c| {
        matches!(
            c.status.as_str(),
            "failure" | "cancelled" | "action-required"
        )
    });
    let pending = counted.iter().any(|c| c.status == "pending");
    let checks = if counted.is_empty() {
        "none"
    } else if pending {
        "pending"
    } else if failing {
        "failing"
    } else {
        "passing"
    };
    PullRequestStanding {
        state: detail.state.clone(),
        checks: checks.to_owned(),
        mergeability: detail.mergeability.clone(),
        url: detail.url.clone(),
    }
}

/// Returns `true` when `standing` satisfies `until`, given how long `checks` has been `"none"`.
pub fn is_pr_settled(
    until: PullRequestUntil,
    standing: &PullRequestStanding,
    without_checks_ms: i64,
) -> bool {
    if standing.state != "open" {
        return true;
    }
    if until != PullRequestUntil::Checks {
        return false;
    }
    if standing.checks == "none" {
        return without_checks_ms >= NO_CHECKS_AFTER_MS;
    }
    standing.checks != "pending"
}

/// Trait abstracting issue trackers and git-host pull request operations (`WorkflowForge`).
pub trait WorkflowForgeDriver: Send + Sync {
    /// Lists issues matching `query`.
    fn issues_list(
        &self,
        query: IssueQuery,
    ) -> BoxFuture<'_, Result<Vec<Value>, WorkflowForgeError>>;
    /// Reads one issue detail JSON payload.
    fn issues_read(&self, issue: IssueAddress) -> BoxFuture<'_, Result<Value, WorkflowForgeError>>;
    /// Posts a comment on an issue.
    fn issues_comment(
        &self,
        issue: IssueAddress,
        body: String,
    ) -> BoxFuture<'_, Result<(), WorkflowForgeError>>;
    /// Transitions an issue to `state`.
    fn issues_set_state(
        &self,
        issue: IssueAddress,
        state: String,
    ) -> BoxFuture<'_, Result<(), WorkflowForgeError>>;
    /// Creates a new issue.
    fn issues_create(
        &self,
        input: IssueCreateInput,
    ) -> BoxFuture<'_, Result<Value, WorkflowForgeError>>;

    /// Lists pull requests matching `query`.
    fn pull_requests_list(
        &self,
        query: PullRequestQuery,
    ) -> BoxFuture<'_, Result<Vec<Value>, WorkflowForgeError>>;
    /// Reads fresh pull-request details.
    fn pull_requests_read(
        &self,
        pull_request: PullRequestAddress,
    ) -> BoxFuture<'_, Result<PullRequestDetail, WorkflowForgeError>>;
    /// Posts a comment on a pull request.
    fn pull_requests_comment(
        &self,
        pull_request: PullRequestAddress,
        body: String,
    ) -> BoxFuture<'_, Result<(), WorkflowForgeError>>;
    /// Executes a pull-request action (`merge`, `close`, `ready`, `update-branch`, ...).
    fn pull_requests_act(
        &self,
        pull_request: PullRequestAddress,
        action: String,
        merge_method: Option<String>,
    ) -> BoxFuture<'_, Result<(), WorkflowForgeError>>;
}

/// Fallback forge driver returning upstream unavailability diagnostics.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnavailableForgeDriver;

impl WorkflowForgeDriver for UnavailableForgeDriver {
    fn issues_list(&self, _q: IssueQuery) -> BoxFuture<'_, Result<Vec<Value>, WorkflowForgeError>> {
        Box::pin(async { Err(forge_unavailable("Issues")) })
    }
    fn issues_read(&self, _i: IssueAddress) -> BoxFuture<'_, Result<Value, WorkflowForgeError>> {
        Box::pin(async { Err(forge_unavailable("Issues")) })
    }
    fn issues_comment(
        &self,
        _i: IssueAddress,
        _b: String,
    ) -> BoxFuture<'_, Result<(), WorkflowForgeError>> {
        Box::pin(async { Err(forge_unavailable("Issues")) })
    }
    fn issues_set_state(
        &self,
        _i: IssueAddress,
        _s: String,
    ) -> BoxFuture<'_, Result<(), WorkflowForgeError>> {
        Box::pin(async { Err(forge_unavailable("Issues")) })
    }
    fn issues_create(
        &self,
        _i: IssueCreateInput,
    ) -> BoxFuture<'_, Result<Value, WorkflowForgeError>> {
        Box::pin(async { Err(forge_unavailable("Issues")) })
    }
    fn pull_requests_list(
        &self,
        _q: PullRequestQuery,
    ) -> BoxFuture<'_, Result<Vec<Value>, WorkflowForgeError>> {
        Box::pin(async { Err(forge_unavailable("Pull requests")) })
    }
    fn pull_requests_read(
        &self,
        _pr: PullRequestAddress,
    ) -> BoxFuture<'_, Result<PullRequestDetail, WorkflowForgeError>> {
        Box::pin(async { Err(forge_unavailable("Pull requests")) })
    }
    fn pull_requests_comment(
        &self,
        _pr: PullRequestAddress,
        _b: String,
    ) -> BoxFuture<'_, Result<(), WorkflowForgeError>> {
        Box::pin(async { Err(forge_unavailable("Pull requests")) })
    }
    fn pull_requests_act(
        &self,
        _pr: PullRequestAddress,
        _a: String,
        _m: Option<String>,
    ) -> BoxFuture<'_, Result<(), WorkflowForgeError>> {
        Box::pin(async { Err(forge_unavailable("Pull requests")) })
    }
}

fn forge_unavailable(what: &str) -> WorkflowForgeError {
    WorkflowForgeError::new(format!(
        "{what} are not available to workflows on this server."
    ))
}
